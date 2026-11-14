// Copyright 2026 TrendVidia LLC
// SPDX-License-Identifier: MIT

// sbe-error-oracle writes what protowire-go's SBE codec says about damaged
// input (#144). It builds inside a protowire-go checkout, so run it through
// scripts/gen-sbe-error-oracle.sh.
//
//	sbe-error-oracle <sbe-test.binpb> <stream.bin> <schema.xml>
//
// Each row is an op, a subject, an argument, an input in hex, then the
// verdict:
//
//	unmarshal  message  limits  bytes   OK, or ERR and the error
//	view       -        limits  bytes   OK, or ERR and the error (Codec.View)
//	call       message  Acc:f   bytes   what the View accessor Acc returns
//	                                    for f: OK and the value, or PANIC and
//	                                    the panic's text. A path a.b reads b
//	                                    of composite a; g#0.f reads f of the
//	                                    first entry of group g
//	stream     Simple   max     bytes   each Decode in turn, "|"-joined: OK,
//	                                    ERR:<error>, ending at EOF or at an
//	                                    error that repeats
//	xml        -        -       text    XMLToProto: OK and the output's
//	                                    bytes (as sbe-oracle writes them), or
//	                                    ERR and the error
//
// The bytes for unmarshal and view are every prefix of a full encoding of
// each message, and hand-made damage to them. A limit is size=N
// (MaxMessageSize) or repeated=N (MaxRepeatedCount), or "-".
package main

import (
	"bufio"
	"bytes"
	"encoding/binary"
	"encoding/hex"
	"errors"
	"fmt"
	"hash/fnv"
	"io"
	"os"
	"strconv"
	"strings"

	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/reflect/protodesc"
	"google.golang.org/protobuf/reflect/protoreflect"
	"google.golang.org/protobuf/types/descriptorpb"
	"google.golang.org/protobuf/types/dynamicpb"

	"github.com/trendvidia/protowire-go/encoding/sbe"
)

func check(err error) {
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(2)
	}
}

func text(err error) string {
	return strings.NewReplacer("\t", " ", "\n", `\n`).Replace(err.Error())
}

func bytesText(b []byte) string {
	if len(b) <= 64 {
		return hex.EncodeToString(b)
	}
	h := fnv.New64a()
	h.Write(b)
	return fmt.Sprintf("%s..len=%d,fnv=%016x", hex.EncodeToString(b[:16]), len(b), h.Sum64())
}

// registry is the fixture's file and its messages by short name.
type registry struct {
	byName map[string]protoreflect.MessageDescriptor
	file   protoreflect.FileDescriptor
}

var files *registry

func load(path string) {
	data, err := os.ReadFile(path)
	check(err)
	var set descriptorpb.FileDescriptorSet
	check(proto.Unmarshal(data, &set))
	fs, err := protodesc.NewFiles(&set)
	check(err)
	fd, err := fs.FindFileByPath("sbe-test.proto")
	check(err)
	files = &registry{byName: map[string]protoreflect.MessageDescriptor{}, file: fd}
	var walk func(protoreflect.MessageDescriptors)
	walk = func(ms protoreflect.MessageDescriptors) {
		for i := 0; i < ms.Len(); i++ {
			files.byName[string(ms.Get(i).Name())] = ms.Get(i)
			walk(ms.Get(i).Messages())
		}
	}
	walk(fd.Messages())
}

func newMsg(name string) *dynamicpb.Message {
	md, ok := files.byName[name]
	if !ok {
		check(fmt.Errorf("no message %s", name))
	}
	return dynamicpb.NewMessage(md)
}

// set assigns name=value on m, by the field's kind.
func set(m protoreflect.Message, name string, val any) {
	fd := m.Descriptor().Fields().ByName(protoreflect.Name(name))
	if fd == nil {
		check(fmt.Errorf("no field %s", name))
	}
	switch v := val.(type) {
	case protoreflect.Message:
		m.Set(fd, protoreflect.ValueOfMessage(v))
	case []protoreflect.Message:
		list := m.Mutable(fd).List()
		for _, e := range v {
			list.Append(protoreflect.ValueOfMessage(e))
		}
	default:
		m.Set(fd, protoreflect.ValueOf(v))
	}
}

func build(name string, kv ...any) protoreflect.Message {
	m := newMsg(name)
	for i := 0; i < len(kv); i += 2 {
		set(m, kv[i].(string), kv[i+1])
	}
	return m
}

func fill(price int64, qty uint32, id uint64) protoreflect.Message {
	return build("Fill", "fill_price", price, "fill_qty", qty, "fill_id", id)
}

// full is one value of each message, every field set.
func full() map[string]protoreflect.Message {
	return map[string]protoreflect.Message{
		"Order": build("Order",
			"order_id", uint64(7), "symbol", "AB€", "price", int64(-5), "quantity", uint32(9),
			"side", protoreflect.EnumNumber(1), "active", true, "weight", 1.5, "score", float32(2.5),
			"fills", []protoreflect.Message{fill(10, 1, 100), fill(-20, 2, 200)}),
		"Simple":        build("Simple", "id", uint32(1), "value", int32(-2)),
		"WithComposite": build("WithComposite", "id", uint64(3), "inner", build("Inner", "x", int64(4), "y", int64(-5)), "code", int32(6)),
		"WithNarrow":    build("WithNarrow", "status", uint32(200), "port", uint32(8080), "delta", int32(-3)),
		"Cut": build("Cut", "text", "abc", "raw", []byte{1, 2}, "code", build("CutCode", "text", "xy"),
			"rows", []protoreflect.Message{build("CutRow", "tag", "r1")}),
	}
}

var order = []string{"Order", "Simple", "WithComposite", "WithNarrow", "Cut"}

func codecFor(limits string) *sbe.Codec {
	var o sbe.CodecOptions
	if limits != "-" {
		name, v, _ := strings.Cut(limits, "=")
		n, err := strconv.Atoi(v)
		check(err)
		switch name {
		case "size":
			o.MaxMessageSize = n
		case "repeated":
			o.MaxRepeatedCount = n
		default:
			check(fmt.Errorf("limit %s", limits))
		}
	}
	c, err := o.NewCodec(files.file)
	check(err)
	return c
}

func u16(b []byte, at int, v uint16) []byte {
	out := append([]byte(nil), b...)
	binary.LittleEndian.PutUint16(out[at:], v)
	return out
}

func splice(b []byte, at int, ins []byte) []byte {
	out := append([]byte(nil), b[:at]...)
	out = append(out, ins...)
	return append(out, b[at:]...)
}

type input struct {
	message, limits string
	data            []byte
}

// damage is the hand-made input, by message, over the full encodings.
func damage(enc map[string][]byte) []input {
	o := enc["Order"]
	// Order: an 8-byte header, its block, then the fills group: a 4-byte
	// header and two entries.
	block := int(binary.LittleEndian.Uint16(o))
	fills := 8 + block
	entry := int(binary.LittleEndian.Uint16(o[fills:]))
	var in []input
	add := func(msg, lim string, b []byte) { in = append(in, input{msg, lim, b}) }
	add("Order", "-", u16(o, 2, 99))                                                               // another template id
	add("Order", "-", u16(o, 4, 9))                                                                // another schema id
	add("Order", "-", u16(o, 0, uint16(block-1)))                                                  // a wire block below the template's
	add("Order", "-", u16(o, 0, uint16(len(o))))                                                   // a wire block past the data's
	add("Order", "-", splice(u16(o, 0, uint16(block+4)), fills, make([]byte, 4)))                  // a longer block, padded: forward compatible
	add("Order", "-", u16(o, fills, uint16(entry-1)))                                              // a group entry below the template's
	add("Order", "-", u16(o, fills, 0))                                                            // a group block of 0, entries declared
	add("Order", "-", u16(u16(o[:fills+4], fills, 3), fills+2, 0))                                 // no entries, the block below the template's
	add("Order", "-", u16(u16(o[:fills+4], fills, 0), fills+2, 0))                                 // no entries, a block of 0
	add("Order", "-", u16(o, fills+2, 3))                                                          // more entries than the data holds
	add("Order", "-", u16(o, fills+2, 65535))                                                      // the most entries a header states
	add("Order", "-", append(u16(u16(o, fills, uint16(entry+2)), fills+2, 1), make([]byte, 2)...)) // a longer entry, padded
	add("Order", "repeated=1", o)
	add("Order", "repeated=2", o)
	add("Order", "size=10", o)
	add("Order", "size="+strconv.Itoa(len(o)), o)
	bad := append([]byte(nil), o...)
	bad[8+8] = 0xff // the symbol's first byte
	add("Order", "-", bad)
	add("Order", "-", enc["Simple"]) // another message's bytes
	add("Simple", "-", o)
	add("Inner", "-", enc["WithComposite"]) // a message with no template
	add("Fill", "-", o)
	add("Simple", "-", nil)
	c := append([]byte(nil), enc["Cut"]...)
	c[8] = 0xc3 // text: a lead byte, then a byte that does not continue it
	add("Cut", "-", c)
	c = append([]byte(nil), enc["Cut"]...)
	c[8+16] = 0xe2 // code.text, cut inside a character
	add("Cut", "-", c)
	c = append([]byte(nil), enc["Cut"]...)
	c[len(c)-8] = 0xff // rows[0].tag
	add("Cut", "-", c)
	return in
}

func unmarshal(in input) string {
	err := codecFor(in.limits).Unmarshal(in.data, newMsg(in.message))
	if err != nil {
		return "ERR\t" + text(err)
	}
	return "OK"
}

func view(in input) string {
	if _, err := codecFor(in.limits).View(in.data); err != nil {
		return "ERR\t" + text(err)
	}
	return "OK"
}

// call reads path with accessor acc from a view of data.
func call(data []byte, acc, path string) (result string) {
	defer func() {
		if r := recover(); r != nil {
			result = "PANIC\t" + fmt.Sprint(r)
		}
	}()
	v, err := codecFor("-").View(data)
	check(err)
	parts := strings.Split(path, ".")
	for _, p := range parts[:len(parts)-1] {
		if g, i, ok := strings.Cut(p, "#"); ok {
			n, err := strconv.Atoi(i)
			check(err)
			v = v.Group(g).Entry(n)
			continue
		}
		v = v.Composite(p)
	}
	f := parts[len(parts)-1]
	var out any
	switch acc {
	case "Int":
		out = v.Int(f)
	case "Uint":
		out = v.Uint(f)
	case "Float":
		out = strconv.FormatFloat(v.Float(f), 'g', -1, 64)
	case "Bool":
		out = v.Bool(f)
	case "Enum":
		out = v.Enum(f)
	case "String":
		out = strconv.Quote(v.String(f))
	case "Bytes":
		out = hex.EncodeToString(v.Bytes(f))
	case "Composite":
		v.Composite(f)
		out = "view"
	case "Len":
		out = v.Group(f).Len()
	default:
		check(fmt.Errorf("accessor %s", acc))
	}
	return fmt.Sprint("OK\t", out)
}

// calls are the accessor reads, by message: each field by its own
// accessor, and the reads that panic.
var calls = map[string][]string{
	"Order": {
		"Uint:order_id", "String:symbol", "Int:price", "Uint:quantity", "Enum:side", "Uint:side",
		"Bool:active", "Uint:active", "Float:weight", "Float:score", "Len:fills",
		"Int:fills#0.fill_price", "Uint:fills#1.fill_qty", "Uint:fills#1.fill_id",
		"Int:nope", "Uint:nope", "Float:nope", "Bool:nope", "Enum:nope", "String:nope",
		"Bytes:nope", "Composite:nope", "Len:nope", "Len:symbol",
		"Int:order_id", "Int:symbol", "Uint:price", "Uint:symbol", "Float:price",
		"Float:symbol", "Enum:symbol", "Enum:weight", "Composite:price", "Composite:symbol",
		"Int:fills", "Int:fills#0.nope",
	},
	"WithComposite": {
		"Uint:id", "Composite:inner", "Int:inner.x", "Int:inner.y", "Int:code",
		"Int:inner.nope", "Composite:inner.x", "Int:inner",
	},
	"WithNarrow": {
		"Uint:status", "Uint:port", "Int:delta", "Int:status", "Uint:delta",
		"Float:status", "Enum:status", "Enum:port", "Enum:delta",
	},
	"Cut": {
		"String:text", "Bytes:raw", "String:code.text", "Len:rows", "String:rows#0.tag",
		"Bytes:text", "String:raw",
	},
}

func stream(codec *sbe.Codec, max int, data []byte) string {
	d := codec.NewDecoder(bytes.NewReader(data))
	if max > 0 {
		d.SetMaxFrameSize(max)
	}
	var out []string
	var last error
	for i := 0; i < 10; i++ {
		err := d.Decode(newMsg("Simple"))
		if err == io.EOF {
			return strings.Join(append(out, "EOF"), "|")
		}
		if err != nil {
			if last != nil && err.Error() == last.Error() {
				return strings.Join(out, "|")
			}
			out = append(out, "ERR:"+text(err))
			last = err
			continue
		}
		last = nil
		out = append(out, "OK")
	}
	check(errors.New("stream did not end"))
	return ""
}

func frame(total uint32, enc uint16, body []byte) []byte {
	b := binary.BigEndian.AppendUint32(nil, total)
	b = binary.BigEndian.AppendUint16(b, enc)
	return append(b, body...)
}

func xmlInputs(schema []byte) [][]byte {
	var in [][]byte
	for n := 0; n <= len(schema); n++ {
		in = append(in, schema[:n])
	}
	head := `<?xml version="1.0" encoding="UTF-8"?>` + "\n"
	root := func(body string) []byte {
		return []byte(head + `<sbe:messageSchema xmlns:sbe="http://fixprotocol.io/2016/sbe" package="x" id="1" version="0" byteOrder="littleEndian">` + body + `</sbe:messageSchema>`)
	}
	for _, s := range []string{
		`<other/>`,
		`<messageSchema package="x" id="nope"/>`,
		`<messageSchema package="x" id="-1"/>`,
		`<messageSchema package="x" id="4294967296"/>`,
		`<messageSchema package="x" id=" 7 "/>`,
		`<messageSchema package="x" id=""/>`,
		`<messageSchema package="x"></other>`,
		`<messageSchema package="x"><types></messageSchema>`,
		`<messageSchema package='x' id='2'/>`,
		`<messageSchema package="x" id="2" id="3"/>`,
		`<messageSchema package="x" id=2/>`,
		`<messageSchema package="x" id="2"`,
		`<messageSchema package="x &amp; y" id="2"/>`,
		`<messageSchema package="x &bogus; y" id="2"/>`,
		`<messageSchema package="x"><!-- note --></messageSchema>`,
		`<messageSchema package="x"><!-- note </messageSchema>`,
		`<messageSchema package="x"><![CDATA[ t ]]></messageSchema>`,
		`<messageSchema package="x"><?pi x?></messageSchema>`,
		`<!DOCTYPE m><messageSchema package="x"/>`,
		`<messageSchema package="x"/><messageSchema package="y"/>`,
		`<messageSchema package="x"/>trailing`,
		`<messageSchema package="x"/><`,
		`<messageSchema package="x"><message name="M" id="x"/></messageSchema>`,
		`<messageSchema package="x"><types><type name="T" primitiveType="char" length="z"/></types></messageSchema>`,
		``,
		"\xff",
		`<messageSchema package="` + "\xff" + `"/>`,
		`junk<messageSchema package="x"/>`,
		`</x>`,
		`<messageSchema package="x"></messageSchema >`,
		`<messageSchema package="x"></messageSchema x>`,
		`< messageSchema/>`,
		`<messageSchema package="x"></ >`,
		`<messageSchema package/>`,
		`<messageSchema package="x" =/>`,
		`<messageSchema package="x"/ >`,
		`<messageSchema package="x &amp y"/>`,
		`<messageSchema package="&#65;&#x42;&#X43;"/>`,
		`<messageSchema package="&#xZZ;"/>`,
		`<messageSchema package="&#1114112;"/>`,
		`<messageSchema package="&#;"/>`,
		`<messageSchema package="&;"/>`,
		`<messageSchema package="&#0;"/>`,
		`<messageSchema package="a<b"/>`,
		`<messageSchema package="a>b"/>`,
		`<!-- a -- b --><messageSchema package="x"/>`,
		`<!-- a - b --><messageSchema package="x"/>`,
		`<!- a --><messageSchema package="x"/>`,
		`<?xml version="1.1"?><messageSchema package="x"/>`,
		`<?xml version="1.0" encoding="ISO-8859-1"?><messageSchema package="x"/>`,
		`<?xml version="1.0" encoding="utf-8"?><messageSchema package="x"/>`,
		`<?xml?><messageSchema package="x"/>`,
		`<?pi?><messageSchema package="x"/>`,
		"<messageSchema package=\"x\">a\x01b</messageSchema>",
		"<messageSchema package=\"a\x01b\"/>",
		"<messageSchema package=\"x\">\ta\r\n</messageSchema>",
		`<messageSchema package="x" id="  "/>`,
		`<messageSchema package="x" id="+5"/>`,
		`<messageSchema package="x" id="007"/>`,
		`<messageSchema package="x" version="1_0"/>`,
		"\n\n<messageSchema package=\"x\">\n</other>",
		"<messageSchema\npackage=\"x\"\n\nid=2/>",
		"<messageSchema package=\"x\">\n\n<types>\n",
		`<a:messageSchema xmlns:a="u" package="x"/>`,
		`<messageSchema package="x"><a:types xmlns:a="u"></b:types></messageSchema>`,
		`<messageSchema package="x"><![CDATA[ <x> ]]></messageSchema>`,
		`<messageSchema package="x"><![CDATA[ a </messageSchema>`,
		`<messageSchema package="x"><!DOCTYPE y></messageSchema>`,
		`<messageSchema package="x"><1/></messageSchema>`,
		`<messageSchema package="x" 1="y"/>`,
		`<messageSchema package="x"><types><type name="A&amp;B" primitiveType="char" length="2"/></types><sbe:message name="M" id="1"><field name="f" id="1" type="A&amp;B"/></sbe:message></messageSchema>`,
		`<messageSchema package="x"><types><enum name="E" encodingType="uint8"><validValue name="A"> 1 &amp; </validValue></enum></types></messageSchema>`,
	} {
		in = append(in, []byte(s))
	}
	for _, body := range []string{
		`<types><type name="T" primitiveType="int16" length="2"/></types><sbe:message name="M" id="1"><field name="f" id="1" type="T"/></sbe:message>`,
		`<types><composite name="C"><type name="t" primitiveType="int64" length="2"/></composite></types>`,
		`<types><type name="T" primitiveType="uint64" length="3"/></types><sbe:message name="M" id="1"><group name="g" id="2"><field name="f" id="1" type="T"/></group></sbe:message>`,
		`<sbe:message name="M" id="1"><field name="f" id="1" type="Missing"/></sbe:message>`,
		`<sbe:message name="M" id="1"><field name="f" id="1" type="int128"/></sbe:message>`,
		`<types><enum name="E" encodingType="uint8"><validValue name="A">0</validValue><validValue name="B">x</validValue></enum></types><sbe:message name="M" id="1"><field name="e" id="1" type="E"/></sbe:message>`,
		`<types><composite name="C"><ref name="r" type="Nope"/></composite></types><sbe:message name="M" id="1"><field name="c" id="1" type="C"/></sbe:message>`,
		`<sbe:message name="M" id="70000"><field name="f" id="1" type="int32"/></sbe:message>`,
		`<sbe:message name="M" id="1"/><sbe:message name="M" id="1"/>`,
	} {
		in = append(in, root(body))
	}
	return in
}

func xmlRow(data []byte) string {
	out, err := sbe.XMLToProto(data)
	if err != nil {
		return "ERR\t" + text(err)
	}
	return "OK\t" + bytesText(out)
}

func main() {
	load(os.Args[1])
	streamData, err := os.ReadFile(os.Args[2])
	check(err)
	schema, err := os.ReadFile(os.Args[3])
	check(err)

	w := bufio.NewWriter(os.Stdout)
	defer w.Flush()
	row := func(op, subject, arg string, data []byte, verdict string) {
		fmt.Fprintf(w, "%s\t%s\t%s\t%s\t%s\n", op, subject, arg, hex.EncodeToString(data), verdict)
	}

	codec := codecFor("-")
	values := full()
	enc := map[string][]byte{}
	for _, name := range order {
		b, err := codec.Marshal(values[name].Interface())
		check(err)
		enc[name] = b
	}
	var inputs []input
	for _, name := range order {
		for n := 0; n <= len(enc[name]); n++ {
			inputs = append(inputs, input{name, "-", enc[name][:n]})
		}
	}
	inputs = append(inputs, damage(enc)...)
	for _, in := range inputs {
		row("unmarshal", in.message, in.limits, in.data, unmarshal(in))
	}
	for _, in := range inputs {
		row("view", "-", in.limits, in.data, view(in))
	}
	for _, name := range order {
		for _, c := range calls[name] {
			acc, path, _ := strings.Cut(c, ":")
			row("call", name, c, enc[name], call(enc[name], acc, path))
		}
	}

	simple := enc["Simple"]
	streams := []struct {
		max  int
		data []byte
	}{}
	for n := 0; n <= len(streamData); n++ {
		streams = append(streams, struct {
			max  int
			data []byte
		}{0, streamData[:n]})
	}
	for _, s := range []struct {
		max  int
		data []byte
	}{
		{0, frame(22, 0x5BE0, simple)},  // big-endian SBE's encoding type
		{0, frame(5, 0xEB50, nil)},      // a total below the header's
		{0, frame(6, 0xEB50, nil)},      // an empty body
		{10, frame(22, 0xEB50, simple)}, // past the frame cap
		{16, frame(22, 0xEB50, simple)}, // at the frame cap
		{0, append(frame(22, 0xEB50, u16(simple, 2, 99)), frame(22, 0xEB50, simple)...)}, // a body that does not decode, then one that does
		{0, append(frame(22, 0xEB50, simple), 0, 0)},                                     // a header cut off
		{0, frame(30, 0xEB50, simple)},                                                   // a body cut off
	} {
		streams = append(streams, s)
	}
	for _, s := range streams {
		row("stream", "Simple", strconv.Itoa(s.max), s.data, stream(codec, s.max, s.data))
	}

	for _, x := range xmlInputs(schema) {
		row("xml", "-", "-", x, xmlRow(x))
	}
}
