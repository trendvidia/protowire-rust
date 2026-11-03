// Copyright 2026 TrendVidia LLC
// SPDX-License-Identifier: MIT

// presence-oracle writes what protowire-go does with each document of
// presence-cases.tsv. It builds inside a protowire-go checkout, so run it
// through scripts/gen-presence-oracle.sh.
//
//	presence-oracle <testdata dir> <cases.tsv>
package main

import (
	"bufio"
	"encoding/hex"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/reflect/protodesc"
	"google.golang.org/protobuf/reflect/protoreflect"
	"google.golang.org/protobuf/reflect/protoregistry"
	"google.golang.org/protobuf/types/descriptorpb"
	"google.golang.org/protobuf/types/dynamicpb"

	"github.com/trendvidia/protowire-go/encoding/pxf"
)

type resolver struct{ files *protoregistry.Files }

func (r resolver) FindMessageByURL(url string) (protoreflect.MessageDescriptor, error) {
	d, err := r.files.FindDescriptorByName(protoreflect.FullName(url[strings.LastIndex(url, "/")+1:]))
	if err != nil {
		return nil, err
	}
	return d.(protoreflect.MessageDescriptor), nil
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

func check(err error) {
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(2)
	}
}

// verdict is "OK", the message's bytes in hex and its sorted null paths, or
// "ERR" and the error, in tab-separated columns.
func verdict(m *dynamicpb.Message, res *pxf.Result, err error) string {
	if err != nil {
		return "ERR\t" + strings.ReplaceAll(err.Error(), "\t", " ") + "\t"
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

func main() {
	dir, cases := os.Args[1], os.Args[2]
	f, err := os.Open(cases)
	check(err)
	defer f.Close()
	out := bufio.NewWriter(os.Stdout)
	defer out.Flush()
	loaded := map[string]*protoregistry.Files{}
	sc := bufio.NewScanner(f)
	for sc.Scan() {
		line := sc.Text()
		c := strings.SplitN(line, "\t", 3)
		files := loaded[c[0]]
		if files == nil {
			files = load(filepath.Join(dir, c[0]))
			loaded[c[0]] = files
		}
		d, err := files.FindDescriptorByName(protoreflect.FullName(c[1]))
		check(err)
		desc := d.(protoreflect.MessageDescriptor)
		opts := pxf.UnmarshalOptions{TypeResolver: resolver{files}}

		full := dynamicpb.NewMessage(desc)
		res, err := opts.UnmarshalFull([]byte(c[2]), full)
		fullRow := verdict(full, res, err)
		plain := dynamicpb.NewMessage(desc)
		plainRow := verdict(plain, nil, opts.Unmarshal([]byte(c[2]), plain))
		fmt.Fprintf(out, "%s\t%s\t%s\n", line, fullRow, plainRow)
	}
	check(sc.Err())
}
