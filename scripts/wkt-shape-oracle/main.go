// Copyright 2026 TrendVidia LLC
// SPDX-License-Identifier: MIT

// wkt-shape-oracle writes what protowire-go does with a message that has a
// well-known type's full name, for each case of wkt-shape-cases.tsv (draft -01
// §wkt-shapes, protowire-go#224). It builds inside a protowire-go checkout, so
// run it through scripts/gen-wkt-shape-oracle.sh.
//
//	wkt-shape-oracle <cases.tsv>
//
// A case is: id, package, message name, message body, the body of the holder
// message H ("-" for a field pw, a list l and a map m of the message), then the
// values to write in each of those. Each case is compiled as one file, as
// protowire-go's own tests compile theirs, and its descriptor set is the first
// row written, so the port reads exactly the descriptors the reference did.
package main

import (
	"bufio"
	"context"
	"encoding/hex"
	"fmt"
	"os"
	"sort"
	"strings"

	"github.com/trendvidia/protocompile"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/reflect/protodesc"
	"google.golang.org/protobuf/reflect/protoreflect"
	"google.golang.org/protobuf/types/descriptorpb"
	"google.golang.org/protobuf/types/dynamicpb"

	"github.com/trendvidia/protowire-go/encoding/pxf"
)

const defaultHolder = "%[1]s pw = 1; repeated %[1]s l = 2; map<string, %[1]s> m = 3;"

func check(err error) {
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(2)
	}
}

func compile(src string) protoreflect.FileDescriptor {
	comp := protocompile.Compiler{
		Resolver: protocompile.WithStandardImports(
			&protocompile.SourceResolver{Accessor: protocompile.SourceAccessorFromMap(map[string]string{"p.proto": src})},
		),
	}
	files, err := comp.Compile(context.Background(), "p.proto")
	check(err)
	return files[0]
}

// oneLine keeps a cell on its row: a tab, a newline and a backslash are
// escaped as Go writes them in a string.
func oneLine(s string) string {
	return strings.NewReplacer(`\`, `\\`, "\t", `\t`, "\n", `\n`).Replace(s)
}

// guard runs f, and turns a panic into a verdict of its own.
func guard(f func() string) (row string) {
	defer func() {
		if r := recover(); r != nil {
			row = "PANIC\t" + oneLine(fmt.Sprint(r)) + "\t"
		}
	}()
	return f()
}

func verdict(m *dynamicpb.Message, res *pxf.Result, err error) string {
	if err != nil {
		return "ERR\t" + oneLine(err.Error()) + "\t"
	}
	b, err := proto.MarshalOptions{Deterministic: true}.Marshal(m)
	check(err)
	var nulls []string
	if res != nil {
		nulls = res.NullFields()
		sort.Strings(nulls)
	}
	return "OK\t" + hex.EncodeToString(b) + "\t" + strings.Join(nulls, ",")
}

// filled is H with every message field holding one empty message: the
// singular field set, the list one element long, the map one entry "k".
func filled(md protoreflect.MessageDescriptor) *dynamicpb.Message {
	m := dynamicpb.NewMessage(md)
	fds := md.Fields()
	for i := range fds.Len() {
		fd := fds.Get(i)
		switch {
		case fd.IsMap():
			if fd.MapValue().Kind() == protoreflect.MessageKind {
				mp := m.Mutable(fd).Map()
				mp.Set(protoreflect.ValueOfString("k").MapKey(), mp.NewValue())
			}
		case fd.IsList():
			if fd.Kind() == protoreflect.MessageKind {
				l := m.Mutable(fd).List()
				l.Append(l.NewElement())
			}
		case fd.Kind() == protoreflect.MessageKind:
			m.Mutable(fd)
		}
	}
	return m
}

func main() {
	f, err := os.Open(os.Args[1])
	check(err)
	defer f.Close()
	out := bufio.NewWriter(os.Stdout)
	defer out.Flush()
	sc := bufio.NewScanner(f)
	for sc.Scan() {
		c := strings.Split(sc.Text(), "\t")
		id, pkg, name, body, holder, values := c[0], c[1], c[2], c[3], c[4], c[5:]
		docs := []string{`s = null`, `s = "a"`}
		if holder == "-" {
			holder = fmt.Sprintf(defaultHolder, name)
			docs = nil
			for _, v := range values {
				docs = append(docs, "pw = "+v, "l = [ "+v+" ]", `m = { "k": `+v+" }")
			}
		}
		src := fmt.Sprintf("syntax = \"proto3\";\npackage %s;\nmessage %s { %s }\nmessage H { %s }\n", pkg, name, body, holder)
		file := compile(src)
		set, err := proto.MarshalOptions{Deterministic: true}.Marshal(&descriptorpb.FileDescriptorSet{
			File: []*descriptorpb.FileDescriptorProto{protodesc.ToFileDescriptorProto(file)},
		})
		check(err)
		md := file.Messages().ByName("H")
		fmt.Fprintf(out, "%s\tfds\t\t%s\n", id, hex.EncodeToString(set))

		var vs []string
		for _, v := range pxf.ValidateDescriptor(md) {
			vs = append(vs, "["+v.Kind.String()+"] "+v.String())
		}
		fmt.Fprintf(out, "%s\tvalidate\t\t%d\t%s\n", id, len(vs), oneLine(strings.Join(vs, " | ")))

		for _, doc := range docs {
			for _, mode := range []struct {
				op   string
				full bool
				skip bool
			}{{"unmarshal", false, false}, {"unmarshal-skip", false, true}, {"full", true, false}, {"full-skip", true, true}} {
				row := guard(func() string {
					m := dynamicpb.NewMessage(md)
					opts := pxf.UnmarshalOptions{SkipValidate: mode.skip}
					if mode.full {
						res, err := opts.UnmarshalFull([]byte(doc), m)
						return verdict(m, res, err)
					}
					return verdict(m, nil, opts.Unmarshal([]byte(doc), m))
				})
				fmt.Fprintf(out, "%s\t%s\t%s\t%s\n", id, mode.op, oneLine(doc), row)
			}
		}

		row := guard(func() string {
			text, err := pxf.Marshal(filled(md))
			if err != nil {
				return "ERR\t" + oneLine(err.Error())
			}
			return "OK\t" + oneLine(string(text))
		})
		fmt.Fprintf(out, "%s\tmarshal\t\t%s\n", id, row)
	}
	check(sc.Err())
}
