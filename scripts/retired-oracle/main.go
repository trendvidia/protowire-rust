// Copyright 2026 TrendVidia LLC
// SPDX-License-Identifier: MIT

// retired-oracle writes what protowire-go does with a descriptor compiled
// before protowire v1.12.0, whose options still sit at the retired extension
// numbers (STABILITY.md promise 3, #133), for each case of retired-cases.tsv.
// It builds inside a protowire-go checkout, so run it through
// scripts/gen-retired-oracle.sh.
//
//	retired-oracle <proto dir> <cases.tsv>
//
// A case is: id, the imports of stale.proto ("-" for none), then the unknown
// bytes on the options of its file, message M, field M.x, oneof M.o, enum E
// and value E_ZERO, each "-" for no options or a list of N=V (a varint) and
// N="s" (a length-delimited field), then "uses" when a second file, uses.proto,
// imports stale.proto and is the one checked. stale.proto is built as
// protowire-go's retired_test.go builds it. Its imports are compiled from
// <proto dir>, the port's vendored annotation files, and the whole set is the
// case's first row: protowire-go's verdicts are read from those bytes, as the
// port reads them.
package main

import (
	"bufio"
	"context"
	"encoding/hex"
	"fmt"
	"os"
	"strconv"
	"strings"

	"github.com/trendvidia/protocompile"
	"google.golang.org/protobuf/encoding/protowire"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/reflect/protodesc"
	"google.golang.org/protobuf/reflect/protoreflect"
	"google.golang.org/protobuf/types/descriptorpb"
	"google.golang.org/protobuf/types/dynamicpb"

	"github.com/trendvidia/protowire-go/encoding/pxf"
	"github.com/trendvidia/protowire-go/encoding/sbe"
)

func check(err error) {
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(2)
	}
}

func oneLine(s string) string {
	return strings.NewReplacer(`\`, `\\`, "\t", `\t`, "\n", `\n`).Replace(s)
}

// unknownBytes reads a list of N=V and N="s" items.
func unknownBytes(spec string) []byte {
	b := []byte{}
	for _, item := range strings.Split(spec, ",") {
		num, val, ok := strings.Cut(item, "=")
		if !ok {
			check(fmt.Errorf("option item %q", item))
		}
		n, err := strconv.Atoi(num)
		check(err)
		if strings.HasPrefix(val, `"`) {
			s, err := strconv.Unquote(val)
			check(err)
			b = protowire.AppendTag(b, protowire.Number(n), protowire.BytesType)
			b = protowire.AppendBytes(b, []byte(s))
			continue
		}
		v, err := strconv.ParseUint(val, 10, 64)
		check(err)
		b = protowire.AppendTag(b, protowire.Number(n), protowire.VarintType)
		b = protowire.AppendVarint(b, v)
	}
	return b
}

// withUnknown returns m with spec's bytes as its unknown fields, or nil for "-".
func withUnknown[M proto.Message](m M, spec string) M {
	var zero M
	if spec == "-" {
		return zero
	}
	m.ProtoReflect().SetUnknown(unknownBytes(spec))
	return m
}

// closure appends fd's imports, then fd, each once.
func closure(fd protoreflect.FileDescriptor, seen map[string]bool, out *[]*descriptorpb.FileDescriptorProto) {
	if seen[fd.Path()] {
		return
	}
	seen[fd.Path()] = true
	imps := fd.Imports()
	for i := range imps.Len() {
		closure(imps.Get(i).FileDescriptor, seen, out)
	}
	*out = append(*out, protodesc.ToFileDescriptorProto(fd))
}

func staleFile(deps []string, c []string) *descriptorpb.FileDescriptorProto {
	fdp := &descriptorpb.FileDescriptorProto{
		Name:       proto.String("stale.proto"),
		Package:    proto.String("stale.v1"),
		Syntax:     proto.String("proto3"),
		Dependency: deps,
		Options:    withUnknown(&descriptorpb.FileOptions{}, c[0]),
		MessageType: []*descriptorpb.DescriptorProto{{
			Name:    proto.String("M"),
			Options: withUnknown(&descriptorpb.MessageOptions{}, c[1]),
			Field: []*descriptorpb.FieldDescriptorProto{{
				Name:       proto.String("x"),
				Number:     proto.Int32(1),
				Label:      descriptorpb.FieldDescriptorProto_LABEL_OPTIONAL.Enum(),
				Type:       descriptorpb.FieldDescriptorProto_TYPE_STRING.Enum(),
				JsonName:   proto.String("x"),
				OneofIndex: proto.Int32(0),
				Options:    withUnknown(&descriptorpb.FieldOptions{}, c[2]),
			}},
			OneofDecl: []*descriptorpb.OneofDescriptorProto{{
				Name:    proto.String("o"),
				Options: withUnknown(&descriptorpb.OneofOptions{}, c[3]),
			}},
		}},
		EnumType: []*descriptorpb.EnumDescriptorProto{{
			Name:    proto.String("E"),
			Options: withUnknown(&descriptorpb.EnumOptions{}, c[4]),
			Value: []*descriptorpb.EnumValueDescriptorProto{{
				Name:    proto.String("E_ZERO"),
				Number:  proto.Int32(0),
				Options: withUnknown(&descriptorpb.EnumValueOptions{}, c[5]),
			}},
		}},
	}
	return fdp
}

// usesFile imports stale.proto and holds an M.
func usesFile() *descriptorpb.FileDescriptorProto {
	return &descriptorpb.FileDescriptorProto{
		Name:       proto.String("uses.proto"),
		Package:    proto.String("uses.v1"),
		Syntax:     proto.String("proto3"),
		Dependency: []string{"stale.proto"},
		MessageType: []*descriptorpb.DescriptorProto{{
			Name: proto.String("U"),
			Field: []*descriptorpb.FieldDescriptorProto{{
				Name: proto.String("m"), Number: proto.Int32(1), JsonName: proto.String("m"),
				Label:    descriptorpb.FieldDescriptorProto_LABEL_OPTIONAL.Enum(),
				Type:     descriptorpb.FieldDescriptorProto_TYPE_MESSAGE.Enum(),
				TypeName: proto.String(".stale.v1.M"),
			}},
		}},
	}
}

func main() {
	protoDir, cases := os.Args[1], os.Args[2]
	comp := protocompile.Compiler{
		Resolver: protocompile.WithStandardImports(&protocompile.SourceResolver{ImportPaths: []string{protoDir}}),
	}
	f, err := os.Open(cases)
	check(err)
	defer f.Close()
	out := bufio.NewWriter(os.Stdout)
	defer out.Flush()
	sc := bufio.NewScanner(f)
	for sc.Scan() {
		c := strings.Split(sc.Text(), "\t")
		id := c[0]
		var deps []string
		if c[1] != "-" {
			deps = strings.Split(c[1], ",")
		}
		var set descriptorpb.FileDescriptorSet
		seen := map[string]bool{}
		if len(deps) > 0 {
			files, err := comp.Compile(context.Background(), deps...)
			check(err)
			for _, fd := range files {
				closure(fd, seen, &set.File)
			}
		}
		set.File = append(set.File, staleFile(deps, c[2:8]))
		target, msg, doc := "stale.proto", "stale.v1.M", `x = "v"`
		if c[8] == "uses" {
			set.File = append(set.File, usesFile())
			target, msg, doc = "uses.proto", "uses.v1.U", `m { x = "v" }`
		}
		raw, err := proto.MarshalOptions{Deterministic: true}.Marshal(&set)
		check(err)
		fmt.Fprintf(out, "%s\tfds\t%s\n", id, hex.EncodeToString(raw))

		// Read back from the bytes, as the port reads them.
		var back descriptorpb.FileDescriptorSet
		check(proto.Unmarshal(raw, &back))
		files, err := protodesc.NewFiles(&back)
		check(err)
		fd, err := files.FindFileByPath(target)
		check(err)

		var vs []string
		for _, v := range pxf.ValidateFile(fd) {
			vs = append(vs, "["+v.Kind.String()+"] "+v.String())
		}
		fmt.Fprintf(out, "%s\tvalidate\t%d\t%s\n", id, len(vs), oneLine(strings.Join(vs, " | ")))

		d, err := files.FindDescriptorByName(protoreflect.FullName(msg))
		check(err)
		m := dynamicpb.NewMessage(d.(protoreflect.MessageDescriptor))
		if err := pxf.Unmarshal([]byte(doc), m); err != nil {
			fmt.Fprintf(out, "%s\tunmarshal\tERR\t%s\n", id, oneLine(err.Error()))
		} else {
			b, err := proto.MarshalOptions{Deterministic: true}.Marshal(m)
			check(err)
			fmt.Fprintf(out, "%s\tunmarshal\tOK\t%s\n", id, hex.EncodeToString(b))
		}

		if _, err := sbe.NewCodec(fd); err != nil {
			fmt.Fprintf(out, "%s\tsbe-codec\tERR\t%s\n", id, oneLine(err.Error()))
		} else {
			fmt.Fprintf(out, "%s\tsbe-codec\tOK\t\n", id)
		}
		if x, err := sbe.ProtoToXML(fd); err != nil {
			fmt.Fprintf(out, "%s\tsbe-xml\tERR\t%s\n", id, oneLine(err.Error()))
		} else {
			fmt.Fprintf(out, "%s\tsbe-xml\tOK\t%s\n", id, oneLine(string(x)))
		}
	}
	check(sc.Err())
}
