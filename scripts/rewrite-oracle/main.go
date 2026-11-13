// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.

// rewrite-oracle runs protowire-go's Rewriter over each case of a case file
// and prints what it makes of it, one line per case:
//
//	<name> TAB <document, base64> TAB <operations, base64> TAB <results, base64>
//
// It is protowire-java's scripts/go-rewrite/main.go (protowire-java#249),
// unchanged but for this comment, and the case file is protowire-java's
// rewrite/cases.txt, so both ports are held to the same rows. The case file
// (crates/protowire-pxf/testdata/conformance/rewrite-cases.txt) holds
// protowire-go's rewriter tests as data: "=== name", "doc <quoted>", then one
// operation per line with quoted arguments. Two operations expand into many
// cases: "corpus" into a round trip and, for every leaf path of the
// document, a Set and a Remove (Go's property tests), and "matrix" into Go's
// TestAnonCommentsSurviveEveryEdit layouts and edits. An operation is its
// fields joined by \x1f, the operations joined by \x1e. The results are each
// operation's ("ok" or "err <text>") and then the document's ("bytes
// <base64>" or "err <text>"), joined by \x1e; a source NewRewriter refuses
// gives "new-err <text>" alone. tests/rewrite_oracle.rs replays every line
// through the Rust Rewriter and holds it to these results. It builds inside a
// protowire-go checkout, so run it through scripts/gen-rewrite-oracle.sh.
package main

import (
	"bufio"
	"encoding/base64"
	"fmt"
	"os"
	"strconv"
	"strings"

	"github.com/trendvidia/protowire-go/encoding/pxf"
)

type kase struct {
	name string
	doc  string
	ops  [][]string
}

func main() {
	if len(os.Args) != 2 {
		fmt.Fprintln(os.Stderr, "usage: rewrite-oracle <cases.txt>")
		os.Exit(2)
	}
	out := bufio.NewWriter(os.Stdout)
	defer out.Flush()
	for _, c := range expand(read(os.Args[1])) {
		fmt.Fprintf(out, "%s\t%s\t%s\t%s\n", c.name, b64(c.doc), b64(encodeOps(c.ops)), b64(run(c)))
	}
}

func b64(s string) string { return base64.StdEncoding.EncodeToString([]byte(s)) }

func encodeOps(ops [][]string) string {
	s := make([]string, len(ops))
	for i, o := range ops {
		s[i] = strings.Join(o, "\x1f")
	}
	return strings.Join(s, "\x1e")
}

func read(path string) []kase {
	data, err := os.ReadFile(path)
	if err != nil {
		fail(err)
	}
	var cs []kase
	for _, line := range strings.Split(string(data), "\n") {
		switch {
		case line == "" || strings.HasPrefix(line, "#"):
		case strings.HasPrefix(line, "=== "):
			cs = append(cs, kase{name: line[4:]})
		default:
			if len(cs) == 0 {
				fail(fmt.Errorf("operation before a case: %q", line))
			}
			f := fields(line)
			c := &cs[len(cs)-1]
			if f[0] == "doc" {
				c.doc = f[1]
			} else {
				c.ops = append(c.ops, f)
			}
		}
	}
	return cs
}

// fields splits an operation line: its name, then quoted arguments.
func fields(line string) []string {
	name, rest, _ := strings.Cut(line, " ")
	f := []string{name}
	for rest = strings.TrimSpace(rest); rest != ""; rest = strings.TrimSpace(rest) {
		q, err := strconv.QuotedPrefix(rest)
		if err != nil {
			fail(fmt.Errorf("argument in %q: %w", line, err))
		}
		s, _ := strconv.Unquote(q)
		f = append(f, s)
		rest = rest[len(q):]
	}
	return f
}

func expand(cs []kase) []kase {
	var out []kase
	for _, c := range cs {
		switch {
		case len(c.ops) == 1 && c.ops[0][0] == "corpus":
			out = append(out, kase{name: c.name + "/roundtrip", doc: c.doc})
			doc, err := pxf.Parse([]byte(c.doc))
			if err != nil {
				fail(err)
			}
			repl := []string{`"edited"`, "12345", "false", "2.75", "[1, 2]"}
			for i, p := range leafPaths(doc) {
				out = append(out, kase{name: c.name + "/set:" + p, doc: c.doc, ops: [][]string{{"set", p, repl[i%len(repl)]}}})
				out = append(out, kase{name: c.name + "/remove:" + p, doc: c.doc, ops: [][]string{{"remove", p}}})
			}
		case len(c.ops) == 1 && c.ops[0][0] == "matrix":
			out = append(out, matrix(c.name)...)
		default:
			out = append(out, c)
		}
	}
	return out
}

// leafPaths is Go's rewrite_test.go leafPaths: the dotted path of every
// leaf assignment and map entry whose segments hold no dot.
func leafPaths(doc *pxf.Document) []string {
	var out []string
	var walk func(prefix string, entries []pxf.Entry)
	walk = func(prefix string, entries []pxf.Entry) {
		for _, e := range entries {
			switch n := e.(type) {
			case *pxf.Assignment:
				if strings.Contains(n.Key, ".") {
					continue
				}
				if bv, ok := n.Value.(*pxf.BlockVal); ok {
					walk(prefix+n.Key+".", bv.Entries)
				} else {
					out = append(out, prefix+n.Key)
				}
			case *pxf.MapEntry:
				if strings.Contains(n.Key, ".") {
					continue
				}
				if bv, ok := n.Value.(*pxf.BlockVal); ok {
					walk(prefix+n.Key+".", bv.Entries)
				} else {
					out = append(out, prefix+n.Key)
				}
			case *pxf.Block:
				if strings.Contains(n.Name, ".") {
					continue
				}
				walk(prefix+n.Name+".", n.Entries)
			}
		}
	}
	walk("", doc.Entries)
	return out
}

// matrix is Go's TestAnonCommentsSurviveEveryEdit: every layout of a
// three-element anonymous list, and every edit of it.
func matrix(name string) []kase {
	keys := []string{"a", "b", "c"}
	var out []kase
	for _, trail := range []string{"", "#", "//", "/*"} {
		for _, between := range []bool{false, true} {
			for _, trailingComma := range []bool{false, true} {
				var sb strings.Builder
				sb.WriteString("f = [\n")
				for i, k := range keys {
					if between {
						sb.WriteString("  // before " + k + "\n")
					}
					s := fmt.Sprintf("  { k = %q }", k)
					if i < len(keys)-1 || trailingComma {
						s += ","
					}
					if trail != "" {
						s += " " + trail + " after " + k
						if trail == "/*" {
							s += " */"
						}
					}
					sb.WriteString(s + "\n")
				}
				sb.WriteString("]\n")
				src := sb.String()
				layout := fmt.Sprintf("%s/%q,%t,%t", name, trail, between, trailingComma)
				add := func(edit string, op []string) {
					out = append(out, kase{name: layout + "/" + edit, doc: src, ops: [][]string{{"keyedby", "f", "k"}, op}})
				}
				for _, k := range keys {
					add("remove "+k, []string{"rmel", "f", k})
					add("insert before "+k, []string{"insel", "f", "new", k, ""})
					add("move to end "+k, []string{"move", "f", k, ""})
					for _, to := range keys {
						if to != k {
							add("move "+k+" before "+to, []string{"move", "f", k, to})
						}
					}
				}
				add("append", []string{"insel", "f", "new", "", ""})
			}
		}
	}
	return out
}

func run(c kase) string {
	r, err := pxf.NewRewriter([]byte(c.doc))
	if err != nil {
		return "new-err " + err.Error()
	}
	var res []string
	for _, o := range c.ops {
		if err := apply(r, o); err != nil {
			res = append(res, "err "+err.Error())
		} else {
			res = append(res, "ok")
		}
	}
	out, err := r.Bytes()
	if err != nil {
		res = append(res, "err "+err.Error())
	} else {
		res = append(res, "bytes "+base64.StdEncoding.EncodeToString(out))
	}
	return strings.Join(res, "\x1e")
}

func value(text string) pxf.Value {
	if text == "<nil>" {
		return nil
	}
	doc, _ := pxf.ParseTolerant([]byte("x = " + text))
	return doc.Entries[0].(*pxf.Assignment).Value
}

func entries(text string) []pxf.Entry {
	if text == "" {
		return nil
	}
	doc, _ := pxf.ParseTolerant([]byte(text))
	return doc.Entries
}

// find is the n-th node a selector names, in a depth-first walk of the
// document: "value:<key>#<n>" an assignment's or map entry's value,
// "block:<name>#<n>" a block. "<unspanned>" is a value built in code, a
// block built in code for "block", and "<nil>" is nil.
func find(r *pxf.Rewriter, sel string) (pxf.Value, *pxf.Block) {
	switch sel {
	case "<nil>":
		return nil, nil
	case "<unspanned>":
		return &pxf.IntVal{Raw: "9"}, &pxf.Block{Name: "x"}
	}
	kind, rest, _ := strings.Cut(sel, ":")
	key, nth, _ := strings.Cut(rest, "#")
	n, _ := strconv.Atoi(nth)
	var gotV pxf.Value
	var gotB *pxf.Block
	var walk func([]pxf.Entry)
	visitValue := func(v pxf.Value) {}
	visitValue = func(v pxf.Value) {
		switch x := v.(type) {
		case *pxf.BlockVal:
			walk(x.Entries)
		case *pxf.ListVal:
			for _, el := range x.Elements {
				visitValue(el)
			}
		}
	}
	walk = func(es []pxf.Entry) {
		for _, e := range es {
			switch x := e.(type) {
			case *pxf.Assignment:
				if kind == "value" && x.Key == key {
					if n == 0 && gotV == nil {
						gotV = x.Value
					}
					n--
				}
				visitValue(x.Value)
			case *pxf.MapEntry:
				if kind == "value" && x.Key == key {
					if n == 0 && gotV == nil {
						gotV = x.Value
					}
					n--
				}
				visitValue(x.Value)
			case *pxf.Block:
				if kind == "block" && x.Name == key {
					if n == 0 && gotB == nil {
						gotB = x
					}
					n--
				}
				walk(x.Entries)
			}
		}
	}
	walk(r.Document().Entries)
	if gotV == nil && gotB == nil {
		fail(fmt.Errorf("selector %q matches nothing", sel))
	}
	return gotV, gotB
}

func apply(r *pxf.Rewriter, o []string) error {
	switch o[0] {
	case "set":
		return r.Set(o[1], value(o[2]))
	case "remove":
		return r.Remove(o[1])
	case "replace":
		old, _ := find(r, o[1])
		return r.ReplaceValue(old, value(o[2]))
	case "setspan":
		start, _ := strconv.Atoi(o[1])
		end, _ := strconv.Atoi(o[2])
		return r.SetSpan(start, end, []byte(o[3]))
	case "append":
		_, b := find(r, o[1])
		var e pxf.Entry
		if o[2] != "<nil>" {
			e = entries(o[2])[0]
		}
		return r.AppendEntry(b, e)
	case "keyedby":
		r.KeyedByField(o[1], o[2])
		return nil
	case "setkeyed":
		return r.SetKeyed(o[1], o[2], o[3], value(o[4]))
	case "rmkeyed":
		return r.RemoveKeyed(o[1], o[2], o[3])
	case "rmel":
		return r.RemoveKeyedElement(o[1], o[2])
	case "insel":
		return r.InsertKeyedElement(o[1], o[2], o[3], entries(o[4]))
	case "rename":
		return r.RenameKeyedElement(o[1], o[2], o[3])
	case "move":
		return r.MoveKeyedElement(o[1], o[2], o[3])
	}
	fail(fmt.Errorf("unknown operation %q", o[0]))
	return nil
}

func fail(err error) {
	fmt.Fprintln(os.Stderr, "rewrite-oracle:", err)
	os.Exit(1)
}
