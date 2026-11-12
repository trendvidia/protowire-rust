// Copyright 2026 TrendVidia LLC
// SPDX-License-Identifier: MIT

// sbe-convert runs protowire-go's SBE XML converter, for the goldens under
// crates/protowire-sbe/testdata/convert/. It builds inside a protowire-go
// checkout, so run it through scripts/gen-sbe-convert.sh.
//
//	sbe-convert xml2proto <schema.xml>             XMLToProto on stdout
//	sbe-convert proto2xml <descriptor set> <file>  ProtoToXML on stdout
//
// A refusal prints "ERR " and the error.
package main

import (
	"fmt"
	"os"

	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/reflect/protodesc"
	"google.golang.org/protobuf/types/descriptorpb"

	"github.com/trendvidia/protowire-go/encoding/sbe"
)

func check(err error) {
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(2)
	}
}

func main() {
	var out []byte
	var err error
	switch os.Args[1] {
	case "xml2proto":
		data, rerr := os.ReadFile(os.Args[2])
		check(rerr)
		out, err = sbe.XMLToProto(data)
	case "proto2xml":
		data, rerr := os.ReadFile(os.Args[2])
		check(rerr)
		var set descriptorpb.FileDescriptorSet
		check(proto.Unmarshal(data, &set))
		files, ferr := protodesc.NewFiles(&set)
		check(ferr)
		fd, ferr := files.FindFileByPath(os.Args[3])
		check(ferr)
		out, err = sbe.ProtoToXML(fd)
	default:
		check(fmt.Errorf("unknown mode %q", os.Args[1]))
	}
	if err != nil {
		fmt.Printf("ERR %s\n", err)
		return
	}
	os.Stdout.Write(out)
}
