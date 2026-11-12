// Copyright 2026 TrendVidia LLC
// SPDX-License-Identifier: MIT

// sbe-oracle writes what protowire-go's SBE codec does with each case of a
// cases file under crates/protowire-sbe/testdata/conformance/. It builds
// inside a protowire-go checkout, so run it through scripts/gen-sbe-oracle.sh.
//
//	sbe-oracle <descriptor set> <cases.tsv>
//
// Each output line is the case's columns, then:
//
//	bind:    OK; or ERR and the error
//	marshal: OK, the bytes, and what Unmarshal reads back (a rendering, or
//	         ERR: and the error); or ERR and the error
//	wire:    OK, what Unmarshal reads, and the bytes Marshal writes for it
//	         again; or ERR and the error
//
// Bytes are hex, or past 64 bytes their first 16 in hex, their length and
// their FNV-1a 64 hash. A rendering lists every field in number order.
package main

import (
	"bufio"
	"encoding/binary"
	"encoding/hex"
	"fmt"
	"hash/fnv"
	"math"
	"os"
	"strconv"
	"strings"

	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/reflect/protodesc"
	"google.golang.org/protobuf/reflect/protoreflect"
	"google.golang.org/protobuf/reflect/protoregistry"
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

func load(path string) *protoregistry.Files {
	data, err := os.ReadFile(path)
	check(err)
	var set descriptorpb.FileDescriptorSet
	check(proto.Unmarshal(data, &set))
	files, err := protodesc.NewFiles(&set)
	check(err)
	return files
}

func bytesText(b []byte) string {
	if len(b) <= 64 {
		return hex.EncodeToString(b)
	}
	h := fnv.New64a()
	h.Write(b)
	return fmt.Sprintf("%s..len=%d,fnv=%016x", hex.EncodeToString(b[:16]), len(b), h.Sum64())
}

func valueText(fd protoreflect.FieldDescriptor, v protoreflect.Value) string {
	switch fd.Kind() {
	case protoreflect.BoolKind:
		return strconv.FormatBool(v.Bool())
	case protoreflect.EnumKind:
		return strconv.Itoa(int(v.Enum()))
	case protoreflect.Int32Kind, protoreflect.Sint32Kind, protoreflect.Sfixed32Kind,
		protoreflect.Int64Kind, protoreflect.Sint64Kind, protoreflect.Sfixed64Kind:
		return strconv.FormatInt(v.Int(), 10)
	case protoreflect.Uint32Kind, protoreflect.Fixed32Kind, protoreflect.Uint64Kind, protoreflect.Fixed64Kind:
		return strconv.FormatUint(v.Uint(), 10)
	case protoreflect.FloatKind:
		return fmt.Sprintf("f32:%08x", math.Float32bits(float32(v.Float())))
	case protoreflect.DoubleKind:
		return fmt.Sprintf("f64:%016x", math.Float64bits(v.Float()))
	case protoreflect.StringKind:
		return fmt.Sprintf("string(%d)", len(v.String()))
	case protoreflect.BytesKind:
		return fmt.Sprintf("bytes(%d)", len(v.Bytes()))
	}
	return render(v.Message())
}

func render(m protoreflect.Message) string {
	fields := m.Descriptor().Fields()
	var parts []string
	for i := 0; i < fields.Len(); i++ {
		fd := fields.Get(i)
		if fd.IsList() {
			list := m.Get(fd).List()
			s := fmt.Sprintf("%s=[%d]", fd.Name(), list.Len())
			if n := list.Len(); n > 0 && n <= 4 {
				var es []string
				for j := 0; j < n; j++ {
					es = append(es, render(list.Get(j).Message()))
				}
				s += strings.Join(es, ",")
			}
			parts = append(parts, s)
			continue
		}
		parts = append(parts, string(fd.Name())+"="+valueText(fd, m.Get(fd)))
	}
	return "{" + strings.Join(parts, " ") + "}"
}

// assign applies path=value;... to m. A path is a field, comp.c for a field
// of a composite, grp+.e to append an entry and set e on it, or e*N to
// append N empty entries.
func assign(m protoreflect.Message, spec string) {
	if spec == "" {
		return
	}
	for _, a := range strings.Split(spec, ";") {
		if name, n, ok := strings.Cut(a, "*"); ok {
			fd := m.Descriptor().Fields().ByName(protoreflect.Name(name))
			count, err := strconv.Atoi(n)
			check(err)
			list := m.Mutable(fd).List()
			for i := 0; i < count; i++ {
				list.Append(list.NewElement())
			}
			continue
		}
		path, val, ok := strings.Cut(a, "=")
		if !ok {
			check(fmt.Errorf("bad assignment %q", a))
		}
		set(m, strings.Split(path, "."), val)
	}
}

func set(m protoreflect.Message, path []string, val string) {
	name := path[0]
	appendEntry := strings.HasSuffix(name, "+")
	fd := m.Descriptor().Fields().ByName(protoreflect.Name(strings.TrimSuffix(name, "+")))
	if fd == nil {
		check(fmt.Errorf("no field %s in %s", name, m.Descriptor().FullName()))
	}
	if len(path) > 1 {
		if appendEntry {
			list := m.Mutable(fd).List()
			el := list.NewElement()
			set(el.Message(), path[1:], val)
			list.Append(el)
			return
		}
		set(m.Mutable(fd).Message(), path[1:], val)
		return
	}
	var v protoreflect.Value
	switch fd.Kind() {
	case protoreflect.BoolKind:
		b, err := strconv.ParseBool(val)
		check(err)
		v = protoreflect.ValueOfBool(b)
	case protoreflect.EnumKind:
		n, err := strconv.ParseInt(val, 10, 32)
		check(err)
		v = protoreflect.ValueOfEnum(protoreflect.EnumNumber(n))
	case protoreflect.Int32Kind, protoreflect.Sint32Kind, protoreflect.Sfixed32Kind:
		n, err := strconv.ParseInt(val, 10, 32)
		check(err)
		v = protoreflect.ValueOfInt32(int32(n))
	case protoreflect.Int64Kind, protoreflect.Sint64Kind, protoreflect.Sfixed64Kind:
		n, err := strconv.ParseInt(val, 10, 64)
		check(err)
		v = protoreflect.ValueOfInt64(n)
	case protoreflect.Uint32Kind, protoreflect.Fixed32Kind:
		n, err := strconv.ParseUint(val, 10, 32)
		check(err)
		v = protoreflect.ValueOfUint32(uint32(n))
	case protoreflect.Uint64Kind, protoreflect.Fixed64Kind:
		n, err := strconv.ParseUint(val, 10, 64)
		check(err)
		v = protoreflect.ValueOfUint64(n)
	case protoreflect.FloatKind:
		f, err := strconv.ParseFloat(val, 32)
		check(err)
		v = protoreflect.ValueOfFloat32(float32(f))
	case protoreflect.DoubleKind:
		f, err := strconv.ParseFloat(val, 64)
		check(err)
		v = protoreflect.ValueOfFloat64(f)
	default:
		check(fmt.Errorf("cannot assign a %s field", fd.Kind()))
	}
	m.Set(fd, v)
}

func errText(err error) string {
	return strings.ReplaceAll(err.Error(), "\t", " ")
}

func main() {
	files := load(os.Args[1])
	f, err := os.Open(os.Args[2])
	check(err)
	defer f.Close()
	out := bufio.NewWriter(os.Stdout)
	defer out.Flush()

	sc := bufio.NewScanner(f)
	for sc.Scan() {
		line := sc.Text()
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}
		cols := strings.Split(line, "\t")
		fd, err := files.FindFileByPath(cols[1])
		check(err)
		codec, err := sbe.NewCodec(fd)
		var result string
		switch cols[0] {
		case "bind":
			if err != nil {
				result = "ERR\t" + errText(err)
			} else {
				result = "OK"
			}
		case "marshal", "wire":
			check(err)
			d, err := files.FindDescriptorByName(protoreflect.FullName(cols[2]))
			check(err)
			md := d.(protoreflect.MessageDescriptor)
			msg := dynamicpb.NewMessage(md)
			if cols[0] == "marshal" {
				assign(msg, cols[3])
				data, err := codec.Marshal(msg)
				if err != nil {
					result = "ERR\t" + errText(err)
					break
				}
				back, err := codec.UnmarshalDescriptor(data, md)
				read := ""
				if err != nil {
					read = "ERR:" + errText(err)
				} else {
					read = render(back)
				}
				result = "OK\t" + bytesText(data) + "\t" + read
				break
			}
			data, err := codec.Marshal(msg)
			check(err)
			bits, err := strconv.ParseUint(cols[3], 16, 64)
			check(err)
			var w [8]byte
			binary.LittleEndian.PutUint64(w[:], bits)
			copy(data[8:], w[:len(data)-8])
			back, err := codec.UnmarshalDescriptor(data, md)
			if err != nil {
				result = "ERR\t" + errText(err)
				break
			}
			again, err := codec.Marshal(back)
			written := ""
			if err != nil {
				written = "ERR:" + errText(err)
			} else {
				written = bytesText(again)
			}
			result = "OK\t" + render(back) + "\t" + written
		default:
			check(fmt.Errorf("unknown op %q", cols[0]))
		}
		fmt.Fprintf(out, "%s\t%s\n", line, result)
	}
	check(sc.Err())
}
