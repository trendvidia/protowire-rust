// Copyright 2026 TrendVidia LLC
// SPDX-License-Identifier: MIT

// stream-oracle writes message streams with protowire-go's own stream
// encoders (#128): a pb stream, an envelope stream and an SBE stream, for
// the port's tests to read and to write byte for byte. It builds inside a
// protowire-go checkout, so run it through scripts/gen-stream-oracle.sh.
//
//	stream-oracle <sbe descriptor set> <pb out> <envelope out> <sbe out>
//
// The messages are fixed here, and each port test spells the same ones.
package main

import (
	"bytes"
	"fmt"
	"os"
	"strings"

	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/reflect/protodesc"
	"google.golang.org/protobuf/reflect/protoreflect"
	"google.golang.org/protobuf/types/descriptorpb"
	"google.golang.org/protobuf/types/dynamicpb"

	"github.com/trendvidia/protowire-go/encoding/pb"
	"github.com/trendvidia/protowire-go/encoding/sbe"
	"github.com/trendvidia/protowire-go/envelope"
)

// StreamMsg is protowire-go's stream_test.go message.
type StreamMsg struct {
	N    int    `protowire:"1"`
	Text string `protowire:"2"`
	Data []byte `protowire:"3"`
}

func check(err error) {
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(2)
	}
}

func main() {
	sbeSet, pbOut, envOut, sbeOut := os.Args[1], os.Args[2], os.Args[3], os.Args[4]

	var buf bytes.Buffer
	enc := pb.NewEncoder(&buf)
	for _, m := range []*StreamMsg{
		{N: 1, Text: "small"},
		{N: 2, Text: strings.Repeat("medium ", 128)},
		{N: 3, Data: bytes.Repeat([]byte{0xab}, 20000)},
		{},
		{N: -5, Text: "neg"},
	} {
		check(enc.Encode(m))
	}
	check(os.WriteFile(pbOut, buf.Bytes(), 0o644))

	buf.Reset()
	eenc := envelope.NewEncoder(&buf)
	for _, e := range []*envelope.Envelope{
		envelope.OK(200, []byte("hi")),
		envelope.Err(400, "E_BAD", "bad %s", "x"),
		envelope.TransportErr("connection reset"),
	} {
		check(eenc.Encode(e))
	}
	check(os.WriteFile(envOut, buf.Bytes(), 0o644))

	data, err := os.ReadFile(sbeSet)
	check(err)
	var set descriptorpb.FileDescriptorSet
	check(proto.Unmarshal(data, &set))
	files, err := protodesc.NewFiles(&set)
	check(err)
	d, err := files.FindDescriptorByName("test.v1.Simple")
	check(err)
	desc := d.(protoreflect.MessageDescriptor)
	codec, err := sbe.NewCodec(desc.ParentFile())
	check(err)
	buf.Reset()
	senc := codec.NewEncoder(&buf)
	for _, v := range []struct {
		id    uint32
		value int32
	}{{1, -100}, {2, 0}, {1 << 20, 2147483647}} {
		m := dynamicpb.NewMessage(desc)
		m.Set(desc.Fields().ByName("id"), protoreflect.ValueOfUint32(v.id))
		m.Set(desc.Fields().ByName("value"), protoreflect.ValueOfInt32(v.value))
		check(senc.Encode(m))
	}
	check(os.WriteFile(sbeOut, buf.Bytes(), 0o644))
}
