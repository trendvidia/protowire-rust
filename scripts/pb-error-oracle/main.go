// Copyright 2026 TrendVidia LLC
// SPDX-License-Identifier: MIT

// pb-error-oracle writes what protowire-go's pb.Unmarshal says about damaged
// input (#144). It builds inside a protowire-go checkout, so run it through
// scripts/gen-pb-error-oracle.sh.
//
// For each message below it encodes one full value with pb.Marshal, then
// decodes every prefix of those bytes, a list of hand-made inputs, and a
// record of every wire type at every field (protowire#614). Each row is the
// message, the limits, the input in hex, then OK and what pb.Marshal writes
// for the decoded value, or ERR and the error. The Rust side
// (crates/protowire/tests/pb_error_oracle.rs) holds structs of the same
// shape, with the same field names.
package main

import (
	"bufio"
	"encoding/hex"
	"fmt"
	"math/big"
	"os"
	"strings"

	"google.golang.org/protobuf/encoding/protowire"

	"github.com/trendvidia/protowire-go/encoding/pb"
	"github.com/trendvidia/protowire-go/envelope"
)

type Inner struct {
	Name string `protowire:"1"`
	N    int64  `protowire:"2"`
}

// All has one field of each kind protowire-go's struct codec reads.
type All struct {
	B     bool              `protowire:"1"`
	I32   int32             `protowire:"2"`
	I64   int64             `protowire:"3"`
	U32   uint32            `protowire:"4"`
	U64   uint64            `protowire:"5"`
	S64   int64             `protowire:"6,zigzag"`
	F32   float32           `protowire:"7"`
	F64   float64           `protowire:"8"`
	Str   string            `protowire:"9"`
	Raw   []byte            `protowire:"10"`
	Msg   *Inner            `protowire:"11"`
	Ints  []int64           `protowire:"12"`
	Strs  []string          `protowire:"13"`
	Kids  []*Inner          `protowire:"14"`
	SMap  map[string]string `protowire:"15"`
	IMap  map[int64]*Inner  `protowire:"16"`
	Big   *big.Int          `protowire:"17"`
	Dec   *big.Rat          `protowire:"18"`
	Float *big.Float        `protowire:"19"`
}

// Node nests itself, for the depth limit.
type Node struct {
	Child *Node `protowire:"1"`
	V     int32 `protowire:"2"`
}

func full() *All {
	return &All{
		B: true, I32: -7, I64: 1 << 40, U32: 300, U64: 1 << 50, S64: -3,
		F32: 1.5, F64: 2.25, Str: "héllo", Raw: []byte{0, 1, 2},
		Msg:   &Inner{Name: "in", N: 5},
		Ints:  []int64{1, 300, -1},
		Strs:  []string{"a", "bc"},
		Kids:  []*Inner{{Name: "k", N: 1}},
		SMap:  map[string]string{"k": "v"},
		IMap:  map[int64]*Inner{2: {Name: "m"}},
		Big:   big.NewInt(-123456789),
		Dec:   big.NewRat(25, 2),
		Float: big.NewFloat(3.5),
	}
}

func envelopeValue() *envelope.Envelope {
	return &envelope.Envelope{
		Status: 404, TransportError: "t", Data: []byte{9, 8},
		Error: &envelope.AppError{
			Code: "NOT_FOUND", Message: "m", Args: []string{"a"},
			Details:  []*envelope.FieldError{{Field: "f", Code: "c", Message: "x", Args: []string{"y"}}},
			Metadata: map[string]string{"k": "v"},
		},
	}
}

// wireRecords returns single records for field num: every wire type, each
// with a well-formed value and a value cut short. It is protowire-go's
// encoding/pb/wire_type_test.go's, as is wireTypeInputs.
func wireRecords(num protowire.Number) [][]byte {
	tag := func(typ protowire.Type) []byte { return protowire.AppendTag(nil, num, typ) }
	cat := func(parts ...[]byte) []byte {
		var b []byte
		for _, p := range parts {
			b = append(b, p...)
		}
		return b
	}
	return [][]byte{
		cat(tag(protowire.VarintType), []byte{0x01}),
		cat(tag(protowire.VarintType), []byte{0x80}), // cut short
		cat(tag(protowire.Fixed64Type), []byte{1, 0, 0, 0, 0, 0, 0, 0}),
		cat(tag(protowire.Fixed64Type), []byte{1, 0, 0}), // cut short
		cat(tag(protowire.BytesType), []byte{0x00}),
		cat(tag(protowire.BytesType), []byte{0x01, 0x01}),
		cat(tag(protowire.BytesType), []byte{0x01}), // cut short
		cat(tag(protowire.StartGroupType), tag(protowire.EndGroupType)),
		tag(protowire.StartGroupType), // never ended
		tag(protowire.EndGroupType),   // ends nothing
		cat(tag(protowire.Fixed32Type), []byte{1, 0, 0, 0}),
		cat(tag(protowire.Fixed32Type), []byte{1, 0}), // cut short
		cat(protowire.AppendVarint(nil, uint64(num)<<3|6), []byte{0x01}),
		cat(protowire.AppendVarint(nil, uint64(num)<<3|7), []byte{0x01}),
	}
}

// wireTypeInputs is every record for each of fields, alone, and carried
// inside each message or map field of inner, which gives the highest field
// number to send there: the message's fields and one it does not have, or
// a map entry's key 1, value 2 and an unknown 3.
func wireTypeInputs(fields []protowire.Number, inner map[protowire.Number]protowire.Number) [][]byte {
	var out [][]byte
	for _, num := range fields {
		out = append(out, wireRecords(num)...)
	}
	for _, outer := range fields {
		last, ok := inner[outer]
		if !ok {
			continue
		}
		for in := protowire.Number(1); in <= last; in++ {
			for _, rec := range wireRecords(in) {
				b := protowire.AppendTag(nil, outer, protowire.BytesType)
				out = append(out, protowire.AppendBytes(b, rec))
			}
		}
	}
	return out
}

// Hand-made inputs, by message.
func handMade() map[string][][]byte {
	tag := func(b []byte, n protowire.Number, t protowire.Type) []byte { return protowire.AppendTag(b, n, t) }
	str := func(n protowire.Number, s string) []byte { return protowire.AppendString(tag(nil, n, protowire.BytesType), s) }
	msg := func(n protowire.Number, body []byte) []byte { return protowire.AppendBytes(tag(nil, n, protowire.BytesType), body) }
	cat := func(parts ...[]byte) []byte { return []byte(strings.Join(func() []string { s := []string{}; for _, p := range parts { s = append(s, string(p)) }; return s }(), "")) }
	overlong := []byte{0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01}
	overflow := []byte{0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x02}
	var all [][]byte
	all = append(all,
		[]byte{0x00},                       // field number 0
		[]byte{0x80},                       // tag cut off
		cat(tag(nil, 99, 7)),               // unknown field, wire type 7
		cat(tag(nil, 99, 6)),               // unknown field, wire type 6
		cat(tag(nil, 99, 0), []byte{0x80}), // unknown varint cut off
		cat(tag(nil, 99, protowire.StartGroupType), tag(nil, 99, protowire.EndGroupType)), // unknown group
		cat(tag(nil, 99, protowire.StartGroupType)),                                      // unknown group never closed
		cat(tag(nil, 99, protowire.EndGroupType)),                                        // stray end group
		cat(tag(nil, 99, protowire.BytesType), []byte{0x05, 0x01}),                       // unknown bytes cut off
		cat(tag(nil, 3, protowire.VarintType), overlong),
		cat(tag(nil, 3, protowire.VarintType), overflow),
		cat(tag(nil, 1, protowire.VarintType), overlong),
		cat(tag(nil, 5, protowire.VarintType), overflow),
		cat(tag(nil, 6, protowire.VarintType), overflow),
		cat(tag(nil, 3, protowire.Fixed64Type), []byte{1, 2, 3, 4, 5, 6, 7, 8}), // int read by kind, not wire type
		cat(tag(nil, 3, 6), []byte{0x01}),                                        // a known field, wire type 6
		cat(tag(nil, 9, 7), []byte{0x01, 'a'}),                                   // a known field, wire type 7
		str(9, "\xff"),
		str(13, "\xff"),
		msg(15, cat(str(1, "\xff"), str(2, "v"))),
		msg(15, cat(str(1, "k"), str(2, "\xff"))),
		msg(15, cat(str(1, "k"), []byte{0x80})),
		msg(15, cat(str(1, "k"), tag(nil, 3, protowire.VarintType), []byte{0x80})),
		msg(15, cat(str(1, "k"), tag(nil, 3, protowire.VarintType), []byte{0x01})),
		msg(15, []byte{0x00}),
		msg(16, cat(tag(nil, 1, protowire.VarintType), overflow)),
		msg(16, cat(tag(nil, 1, protowire.VarintType), []byte{0x02}, msg(2, str(1, "\xff")))),
		msg(11, cat(tag(nil, 2, protowire.VarintType), overflow)),
		msg(11, str(1, "\xff")),
		msg(14, str(1, "\xff")),
		msg(12, []byte{0x01, 0x80}),             // packed element cut off
		cat(tag(nil, 12, protowire.BytesType), []byte{0x05, 0x01}), // packed body cut off
		msg(17, []byte{0x00}),
		msg(17, cat(tag(nil, 1, protowire.BytesType), []byte{0x05})),
		msg(17, cat(tag(nil, 2, protowire.VarintType), []byte{0x80})),
		msg(17, cat(tag(nil, 9, protowire.VarintType), []byte{0x80})),
		msg(18, []byte{0x00}),
		msg(18, cat(tag(nil, 1, protowire.BytesType), []byte{0x05})),
		msg(18, cat(tag(nil, 2, protowire.VarintType), []byte{0x80})),
		msg(18, cat(tag(nil, 3, protowire.VarintType), []byte{0x80})),
		msg(18, cat(tag(nil, 9, protowire.VarintType), []byte{0x80})),
		msg(19, []byte{0x00}),
		msg(19, cat(tag(nil, 1, protowire.BytesType), []byte{0x05})),
		msg(19, cat(tag(nil, 2, protowire.VarintType), []byte{0x80})),
		msg(19, cat(tag(nil, 3, protowire.VarintType), []byte{0x80})),
		msg(19, cat(tag(nil, 4, protowire.VarintType), []byte{0x80})),
		msg(19, cat(tag(nil, 9, protowire.VarintType), []byte{0x80})),
	)
	env := [][]byte{
		cat(tag(nil, 1, protowire.VarintType), overflow),
		str(2, "\xff"),
		msg(4, str(1, "\xff")),
		msg(4, msg(4, str(1, "\xff"))),
		msg(4, msg(5, cat(str(1, "k"), str(2, "\xff")))),
		msg(4, msg(5, []byte{0x00})),
		msg(4, cat(tag(nil, 3, protowire.BytesType), []byte{0x05})),
	}
	all = append(all, wireTypeInputs(
		[]protowire.Number{1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20},
		map[protowire.Number]protowire.Number{11: 3, 14: 3, 15: 3, 16: 3, 17: 3, 18: 4, 19: 5},
	)...)
	// A matching record after a mismatched one: the mismatch leaves the
	// field as it was, and the match still sets it.
	all = append(all,
		cat(tag(nil, 3, protowire.VarintType), []byte{0x07}, tag(nil, 3, protowire.Fixed32Type), []byte{1, 0, 0, 0}),
		cat(str(9, "a"), tag(nil, 9, protowire.VarintType), []byte{0x01}),
		cat(msg(12, []byte{0x01}), tag(nil, 12, protowire.VarintType), []byte{0x02}),
		cat(tag(nil, 12, protowire.VarintType), []byte{0x01}, tag(nil, 12, protowire.Fixed32Type), []byte{1, 0, 0, 0}),
		cat(tag(nil, 12, protowire.VarintType), []byte{0x01}, tag(nil, 12, protowire.StartGroupType), tag(nil, 12, protowire.EndGroupType)),
		msg(15, cat(str(1, "k"), tag(nil, 1, protowire.VarintType), []byte{0x01}, str(2, "v"), tag(nil, 2, protowire.Fixed64Type), []byte{1, 0, 0, 0, 0, 0, 0, 0})),
		msg(16, cat(tag(nil, 1, protowire.VarintType), []byte{0x05}, tag(nil, 2, protowire.VarintType), []byte{0x01})),
		msg(17, cat(tag(nil, 1, protowire.BytesType), []byte{0x01, 0x05}, tag(nil, 2, protowire.BytesType), []byte{0x01, 0x01})),
		msg(18, cat(tag(nil, 1, protowire.BytesType), []byte{0x01, 0x0f}, tag(nil, 2, protowire.Fixed32Type), []byte{1, 0, 0, 0})),
		msg(19, cat(tag(nil, 1, protowire.BytesType), []byte{0x01, 0x01}, tag(nil, 2, protowire.BytesType), []byte{0x01, 0x01}, tag(nil, 3, protowire.VarintType), []byte{0x35})),
		// Wire types 6 and 7 inside a skipped group, and a group that
		// holds a field number past 2^29-1, which a skip reads as protobuf
		// does.
		cat(tag(nil, 99, protowire.StartGroupType), tag(nil, 1, 6), tag(nil, 99, protowire.EndGroupType)),
		cat(tag(nil, 99, protowire.StartGroupType), protowire.AppendVarint(nil, 1<<32|0), []byte{0x01}, tag(nil, 99, protowire.EndGroupType)),
		cat(tag(nil, 99, protowire.StartGroupType), protowire.AppendVarint(nil, uint64(1<<31)<<3), []byte{0x01}, tag(nil, 99, protowire.EndGroupType)),
		cat(tag(nil, 99, protowire.StartGroupType), protowire.AppendVarint(nil, 0), tag(nil, 99, protowire.EndGroupType)),
		cat(tag(nil, 99, protowire.StartGroupType), tag(nil, 1, protowire.VarintType), overflow, tag(nil, 99, protowire.EndGroupType)),
		cat(tag(nil, 99, protowire.StartGroupType), tag(nil, 1, protowire.EndGroupType)),
		cat(tag(nil, 99, protowire.VarintType), overflow),
	)
	env = append(env, wireTypeInputs(
		[]protowire.Number{1, 2, 3, 4, 5},
		map[protowire.Number]protowire.Number{4: 6},
	)...)
	deep := []byte{}
	for i := 0; i < 101; i++ {
		deep = msg(1, deep)
	}
	return map[string][][]byte{"All": all, "Envelope": env, "Node": {deep, msg(1, msg(1, cat(tag(nil, 2, protowire.VarintType), overflow)))}}
}

// decode returns the decoded value, or the error.
func decode(name string, lim pb.UnmarshalOptions, data []byte) (any, error) {
	var v any
	switch name {
	case "All":
		v = &All{}
	case "Envelope":
		v = &envelope.Envelope{}
	case "Node":
		v = &Node{}
	default:
		panic(name)
	}
	return v, lim.Unmarshal(data, v)
}

func main() {
	out := bufio.NewWriter(os.Stdout)
	defer out.Flush()
	row := func(name, limits string, lim pb.UnmarshalOptions, data []byte) {
		var verdict string
		if v, err := decode(name, lim, data); err != nil {
			// protobuf-go writes "proto: " with a no-break space in some
			// binaries, chosen by a hash of the binary; the plain space is
			// the text.
			text := strings.ReplaceAll(err.Error(), "proto:\u00a0", "proto: ")
			verdict = "ERR\t" + strings.ReplaceAll(text, "\n", `\n`)
		} else {
			// What the decoded value holds, as pb.Marshal writes it. A map
			// holds at most one entry here, so the bytes are stable.
			out, err := pb.Marshal(v)
			if err != nil {
				panic(err)
			}
			verdict = "OK\t" + hex.EncodeToString(out)
		}
		fmt.Fprintf(out, "%s\t%s\t%s\t%s\n", name, limits, hex.EncodeToString(data), verdict)
	}
	for _, v := range []struct {
		name string
		val  any
	}{{"All", full()}, {"Envelope", envelopeValue()}} {
		data, err := pb.Marshal(v.val)
		if err != nil {
			panic(err)
		}
		for n := 0; n <= len(data); n++ {
			row(v.name, "-", pb.UnmarshalOptions{}, data[:n])
		}
	}
	hm := handMade()
	for _, name := range []string{"All", "Envelope", "Node"} {
		for _, data := range hm[name] {
			row(name, "-", pb.UnmarshalOptions{}, data)
		}
	}
	data, err := pb.Marshal(full())
	if err != nil {
		panic(err)
	}
	row("All", "repeated=2", pb.UnmarshalOptions{MaxRepeatedCount: 2}, data)
	row("All", "size=10", pb.UnmarshalOptions{MaxMessageSize: 10}, data)
	row("Node", "depth=1", pb.UnmarshalOptions{MaxNestingDepth: 1}, hm["Node"][1])
}
