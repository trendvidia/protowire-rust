// Copyright 2026 TrendVidia LLC
// SPDX-License-Identifier: MIT

// keyed-oracle writes what protowire-go does with the keyed repeated fields
// of each case of keyed-cases.tsv (#130): the typed KeyedError of a decode,
// and KeyedDiagnostics over the parsed document. It builds inside a
// protowire-go checkout, so run it through scripts/gen-keyed-oracle.sh.
//
//	keyed-oracle <testdata dir> <cases.tsv>
//
// A case is: descriptor set, message, op, document.
//
//	doc    Unmarshal, UnmarshalFull, then Parse and KeyedDiagnostics
//	scan   NewDatasetReader and Scan of every row
//
// Each verdict is OK; KEYED and the KeyedError's kind, field, key,
// position and text; or ERR and the error's text. A diagnostics verdict
// is each diagnostic as kind|field|key|line:col|offset|msg, joined by
// " ; ", or PARSE and Parse's error. ParseTolerant must agree with Parse:
// the oracle exits if it reports errors on a document Parse reads.
package main

import (
	"bufio"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
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

func unescape(s string) string {
	return strings.NewReplacer(`\\`, `\`, `\t`, "\t", `\n`, "\n").Replace(s)
}

func keyed(k pxf.KeyedError) string {
	return fmt.Sprintf("%s|%s|%s|%d:%d|%d|%s", k.Kind, k.Field, k.Key, k.Pos.Line, k.Pos.Column, k.Pos.Offset, k.Msg)
}

func verdict(err error) string {
	if err == nil {
		return "OK"
	}
	var kerr *pxf.KeyedError
	if errors.As(err, &kerr) {
		return "KEYED " + keyed(*kerr) + " " + err.Error()
	}
	return "ERR " + err.Error()
}

func main() {
	dir, cases := os.Args[1], os.Args[2]
	f, err := os.Open(cases)
	check(err)
	defer f.Close()
	pools := map[string]*protoregistry.Files{}
	sc := bufio.NewScanner(f)
	sc.Buffer(make([]byte, 1<<20), 1<<20)
	for sc.Scan() {
		line := sc.Text()
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}
		cols := strings.Split(line, "\t")
		if len(cols) != 4 {
			check(fmt.Errorf("want 4 columns: %q", line))
		}
		fds, msgName, op, doc := cols[0], cols[1], cols[2], unescape(cols[3])
		files, ok := pools[fds]
		if !ok {
			files = load(filepath.Join(dir, fds))
			pools[fds] = files
		}
		d, err := files.FindDescriptorByName(protoreflect.FullName(msgName))
		check(err)
		desc := d.(protoreflect.MessageDescriptor)
		opts := pxf.UnmarshalOptions{TypeResolver: resolver{files}}
		var out []string
		switch op {
		case "doc":
			err := opts.Unmarshal([]byte(doc), dynamicpb.NewMessage(desc))
			out = append(out, verdict(err))
			_, err = opts.UnmarshalFull([]byte(doc), dynamicpb.NewMessage(desc))
			out = append(out, verdict(err))
			parsed, perr := pxf.Parse([]byte(doc))
			if perr != nil {
				out = append(out, "PARSE "+perr.Error())
				break
			}
			tol, errs := pxf.ParseTolerant([]byte(doc))
			if len(errs) != 0 {
				check(fmt.Errorf("ParseTolerant reports %v on %q, which Parse reads", errs, doc))
			}
			diags := pxf.KeyedDiagnostics(parsed, desc)
			again := pxf.KeyedDiagnostics(tol, desc)
			if fmt.Sprint(diags) != fmt.Sprint(again) {
				check(fmt.Errorf("diagnostics differ between Parse and ParseTolerant on %q", doc))
			}
			var ds []string
			for _, k := range diags {
				ds = append(ds, keyed(k))
			}
			if len(ds) == 0 {
				out = append(out, "NONE")
			} else {
				out = append(out, strings.Join(ds, " ; "))
			}
		case "scan":
			tr, err := opts.NewDatasetReader(strings.NewReader(doc))
			if err != nil {
				out = append(out, "NEW "+verdict(err), "-", "-")
				break
			}
			rows := 0
			for {
				m := dynamicpb.NewMessage(desc)
				err := tr.Scan(m)
				if err == io.EOF {
					out = append(out, fmt.Sprintf("OK %d", rows), "-", "-")
					break
				}
				if err != nil {
					out = append(out, fmt.Sprintf("ROW %d %s", rows, verdict(err)), "-", "-")
					break
				}
				rows++
			}
		default:
			check(fmt.Errorf("unknown op %q", op))
		}
		fmt.Print(line)
		for _, v := range out {
			fmt.Print("\t" + oneLine(v))
		}
		fmt.Println()
	}
	check(sc.Err())
}
