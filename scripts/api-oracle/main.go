// Copyright 2026 TrendVidia LLC
// SPDX-License-Identifier: MIT

// api-oracle writes what protowire-go's pxf API does with each case of
// api-cases.tsv (#127). It builds inside a protowire-go checkout, so run it
// through scripts/gen-api-oracle.sh.
//
//	api-oracle <testdata dir> <cases.tsv>
//
// A case is: descriptor set, message, op, then the op's arguments:
//
//	full <doc>             UnmarshalFull
//	full-skip <doc>        UnmarshalFull with SkipPostDecode
//	marshal <doc>          Unmarshal, then Marshal
//	secret <doc>           UnmarshalFull with an OnSecretField hook that
//	                       records each call; secret-plain with Unmarshal;
//	                       secret-err with a hook that fails
//	format <doc>           Parse, then FormatValue of each top-level
//	                       assignment's value, joined by " | "
//	apply <field> <lit>    ApplyDefault on a new message
//	required <field>       IsRequired
//	default <field>        Default
//
// Each row is the case, then the verdict: OK and the message's pb hex, its
// sorted PresentFields and its sorted NullFields (the last two only for a
// full decode), or for marshal the text, for format the values, for the
// secret ops the hook's calls, "path=value" joined by " ; ", last; or ERR and the error; or, for
// required and default, the answer.
package main

import (
	"bufio"
	"encoding/hex"
	"errors"
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

func oneLine(s string) string {
	return strings.NewReplacer(`\`, `\\`, "\t", `\t`, "\n", `\n`).Replace(s)
}

func sorted(paths []string) string {
	sort.Strings(paths)
	return strings.Join(paths, ",")
}

func bytesOf(m *dynamicpb.Message) string {
	b, err := proto.MarshalOptions{Deterministic: true}.Marshal(m)
	check(err)
	return hex.EncodeToString(b)
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
		c := strings.Split(line, "\t")
		files := loaded[c[0]]
		if files == nil {
			files = load(filepath.Join(dir, c[0]))
			loaded[c[0]] = files
		}
		d, err := files.FindDescriptorByName(protoreflect.FullName(c[1]))
		check(err)
		desc := d.(protoreflect.MessageDescriptor)
		field := func() protoreflect.FieldDescriptor {
			fd := desc.Fields().ByName(protoreflect.Name(c[3]))
			if fd == nil {
				check(fmt.Errorf("%s has no field %s", c[1], c[3]))
			}
			return fd
		}
		var row string
		switch c[2] {
		case "full", "full-skip":
			m := dynamicpb.NewMessage(desc)
			res, err := pxf.UnmarshalOptions{SkipPostDecode: c[2] == "full-skip"}.UnmarshalFull([]byte(c[3]), m)
			if err != nil {
				row = "ERR\t" + oneLine(err.Error()) + "\t\t"
			} else {
				row = "OK\t" + bytesOf(m) + "\t" + sorted(res.PresentFields()) + "\t" + sorted(res.NullFields())
			}
		case "marshal":
			m := dynamicpb.NewMessage(desc)
			if err := pxf.Unmarshal([]byte(c[3]), m); err != nil {
				row = "ERR\t" + oneLine(err.Error())
			} else if text, err := pxf.Marshal(m); err != nil {
				row = "ERR\t" + oneLine(err.Error())
			} else {
				row = "OK\t" + oneLine(string(text))
			}
		case "format":
			doc, err := pxf.Parse([]byte(c[3]))
			if err != nil {
				row = "ERR\t" + oneLine(err.Error())
				break
			}
			var vals []string
			for _, e := range doc.Entries {
				if a, ok := e.(*pxf.Assignment); ok {
					vals = append(vals, string(pxf.FormatValue(a.Value)))
				}
			}
			row = "OK\t" + oneLine(strings.Join(vals, " | "))
		case "secret", "secret-plain", "secret-err":
			var calls []string
			opts := pxf.UnmarshalOptions{
				TypeResolver: resolver{files},
				OnSecretField: func(path, value string) error {
					calls = append(calls, path+"="+value)
					if c[2] == "secret-err" {
						return errors.New("vault closed")
					}
					return nil
				},
			}
			m := dynamicpb.NewMessage(desc)
			var res *pxf.Result
			var err error
			if c[2] == "secret-plain" {
				err = opts.Unmarshal([]byte(c[3]), m)
			} else {
				res, err = opts.UnmarshalFull([]byte(c[3]), m)
			}
			joined := oneLine(strings.Join(calls, " ; "))
			switch {
			case err != nil:
				row = "ERR\t" + oneLine(err.Error()) + "\t\t" + joined
			case res != nil:
				row = "OK\t" + bytesOf(m) + "\t" + sorted(res.PresentFields()) + "\t" + joined
			default:
				row = "OK\t" + bytesOf(m) + "\t\t" + joined
			}
		case "apply":
			m := dynamicpb.NewMessage(desc)
			if err := pxf.ApplyDefault(m, field(), c[4]); err != nil {
				row = "ERR\t" + oneLine(err.Error())
			} else {
				row = "OK\t" + bytesOf(m)
			}
		case "required":
			row = fmt.Sprint(pxf.IsRequired(field()))
		case "default":
			lit, ok := pxf.Default(field())
			row = fmt.Sprintf("%v\t%s", ok, oneLine(lit))
		default:
			check(fmt.Errorf("op %q", c[2]))
		}
		fmt.Fprintf(out, "%s\t%s\n", line, row)
	}
	check(sc.Err())
}
