// Copyright 2026 TrendVidia LLC
// SPDX-License-Identifier: MIT

// tolerant-oracle writes what protowire-go's Parse and ParseTolerant do with
// each document of tolerant-cases.tsv (#130). It builds inside a protowire-go
// checkout, so run it through scripts/gen-tolerant-oracle.sh.
//
//	tolerant-oracle <testdata dir> <cases.tsv>
//
// A case is a document, escaped, or `file <path>` for a file under the
// testdata directory, or `prefixes <path>` for every prefix of one, a case
// each. Each row is the case, then: Parse's verdict (OK, or ERR and the
// error); ParseTolerant's errors, each line:col:offset|msg, joined by " ; "
// (NONE for none); and the tree ParseTolerant returned, in the dump format
// below, which tests/tolerant_oracle.rs writes the same way.
package main

import (
	"bufio"
	"fmt"
	"os"
	"path/filepath"
	"reflect"
	"strings"

	"github.com/trendvidia/protowire-go/encoding/pxf"
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

func unescape(s string) string {
	return strings.NewReplacer(`\\`, `\`, `\t`, "\t", `\n`, "\n").Replace(s)
}

// q writes free text: printable ASCII but '\' and '"' as is, every other
// byte as \xHH, in quotes.
func q(s string) string {
	var b strings.Builder
	b.WriteByte('"')
	for i := 0; i < len(s); i++ {
		c := s[i]
		if c >= 0x20 && c < 0x7f && c != '\\' && c != '"' {
			b.WriteByte(c)
		} else {
			fmt.Fprintf(&b, `\x%02x`, c)
		}
	}
	b.WriteByte('"')
	return b.String()
}

func pos(p pxf.Position) string { return fmt.Sprintf("%d:%d:%d", p.Line, p.Column, p.Offset) }

func bit(b bool) string {
	if b {
		return "1"
	}
	return "0"
}

func comments(cs []pxf.Comment) string {
	var out []string
	for _, c := range cs {
		out = append(out, "c("+pos(c.Pos)+","+q(c.Text)+")")
	}
	return "[" + strings.Join(out, ",") + "]"
}

func value(v pxf.Value) string {
	switch v := v.(type) {
	case nil:
		return "_"
	case *pxf.StringVal:
		return "s(" + pos(v.Pos) + "," + q(v.Value) + ")"
	case *pxf.IntVal:
		return "i(" + pos(v.Pos) + "," + q(v.Raw) + "," + q(v.Prec) + ")"
	case *pxf.FloatVal:
		return "f(" + pos(v.Pos) + "," + q(v.Raw) + "," + q(v.Prec) + ")"
	case *pxf.BoolVal:
		return "bo(" + pos(v.Pos) + "," + bit(v.Value) + ")"
	case *pxf.BytesVal:
		return "by(" + pos(v.Pos) + "," + q(string(v.Value)) + ")"
	case *pxf.NullVal:
		return "n(" + pos(v.Pos) + ")"
	case *pxf.IdentVal:
		return "id(" + pos(v.Pos) + "," + q(v.Name) + ")"
	case *pxf.TimestampVal:
		return "ts(" + pos(v.Pos) + "," + q(v.Raw) + ")"
	case *pxf.DurationVal:
		return "du(" + pos(v.Pos) + "," + q(v.Raw) + ")"
	case *pxf.ListVal:
		var out []string
		for _, e := range v.Elements {
			out = append(out, value(e))
		}
		return "l(" + pos(v.Pos) + ",[" + strings.Join(out, ",") + "])"
	case *pxf.BlockVal:
		return "bl(" + pos(v.Pos) + "," + entries(v.Entries) + "," + comments(v.TrailingComments) + ")"
	case *pxf.BadVal:
		return "bad(" + pos(v.Pos) + ")"
	default:
		check(fmt.Errorf("unknown value %T", v))
		return ""
	}
}

func entries(es []pxf.Entry) string {
	var out []string
	for _, e := range es {
		switch e := e.(type) {
		case *pxf.Assignment:
			out = append(out, "a("+pos(e.Pos)+","+q(e.Key)+","+bit(e.KeyQuoted)+","+value(e.Value)+","+comments(e.LeadingComments)+","+q(e.TrailingComment)+")")
		case *pxf.MapEntry:
			out = append(out, "m("+pos(e.Pos)+","+q(e.Key)+","+bit(e.KeyQuoted)+","+value(e.Value)+","+comments(e.LeadingComments)+","+q(e.TrailingComment)+")")
		case *pxf.Block:
			out = append(out, "b("+pos(e.Pos)+","+q(e.Name)+","+bit(e.NameQuoted)+","+entries(e.Entries)+","+comments(e.LeadingComments)+","+comments(e.TrailingComments)+")")
		case *pxf.TypeEntry:
			out = append(out, "ty("+pos(e.Pos)+","+q(e.URL)+","+comments(e.LeadingComments)+","+q(e.TrailingComment)+")")
		default:
			check(fmt.Errorf("unknown entry %T", e))
		}
	}
	return "[" + strings.Join(out, ",") + "]"
}

func strs(ss []string) string {
	var out []string
	for _, s := range ss {
		out = append(out, q(s))
	}
	return "[" + strings.Join(out, ",") + "]"
}

func dump(d *pxf.Document) string {
	var dirs, sets, protos []string
	for _, x := range d.Directives {
		body := "-"
		if x.Body != nil {
			body = q(string(x.Body))
		}
		dirs = append(dirs, "d("+pos(x.Pos)+","+q(x.Name)+","+strs(x.Prefixes)+","+q(x.Type)+","+body+","+comments(x.LeadingComments)+")")
	}
	for _, x := range d.Datasets {
		var rows []string
		for _, r := range x.Rows {
			var cells []string
			for _, c := range r.Cells {
				cells = append(cells, value(c))
			}
			rows = append(rows, "r("+pos(r.Pos)+",["+strings.Join(cells, ",")+"],"+comments(r.LeadingComments)+")")
		}
		sets = append(sets, "t("+pos(x.Pos)+","+q(x.Type)+","+strs(x.Columns)+",["+strings.Join(rows, ",")+"],"+comments(x.LeadingComments)+")")
	}
	for _, x := range d.Protos {
		protos = append(protos, "p("+pos(x.Pos)+","+x.Shape.String()+","+q(x.TypeName)+","+q(string(x.Body))+","+comments(x.LeadingComments)+")")
	}
	return "doc(" + q(d.TypeURL) + "," + fmt.Sprint(d.BodyOffset) + "," + comments(d.LeadingComments) +
		",[" + strings.Join(dirs, ",") + "],[" + strings.Join(sets, ",") + "],[" + strings.Join(protos, ",") +
		"]," + entries(d.Entries) + "," + comments(d.TrailingComments) + ")"
}

func row(label, doc string) {
	verdict := "OK"
	strict, err := pxf.Parse([]byte(doc))
	if err != nil {
		verdict = "ERR " + err.Error()
	}
	tol, errs := pxf.ParseTolerant([]byte(doc))
	var es []string
	for _, e := range errs {
		es = append(es, pos(e.Pos)+"|"+e.Msg)
	}
	tolErrs := "NONE"
	if len(es) > 0 {
		tolErrs = strings.Join(es, " ; ")
	}
	if err == nil {
		// ParseTolerant's own contract: a document Parse reads gives the
		// same tree and no error.
		if len(errs) != 0 || !reflect.DeepEqual(strict, tol) {
			check(fmt.Errorf("%s: ParseTolerant differs from Parse on a valid document", label))
		}
	}
	fmt.Println(label + "\t" + oneLine(verdict) + "\t" + oneLine(tolErrs) + "\t" + oneLine(dump(tol)))
}

func main() {
	dir, cases := os.Args[1], os.Args[2]
	f, err := os.Open(cases)
	check(err)
	defer f.Close()
	sc := bufio.NewScanner(f)
	sc.Buffer(make([]byte, 1<<20), 1<<20)
	for sc.Scan() {
		line := sc.Text()
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}
		switch {
		case strings.HasPrefix(line, "file "):
			data, err := os.ReadFile(filepath.Join(dir, line[len("file "):]))
			check(err)
			row(line, string(data))
		case strings.HasPrefix(line, "prefixes "):
			data, err := os.ReadFile(filepath.Join(dir, line[len("prefixes "):]))
			check(err)
			for i := 0; i <= len(data); i++ {
				row(fmt.Sprintf("%s %d", line, i), string(data[:i]))
			}
		default:
			row(line, unescape(line))
		}
	}
	check(sc.Err())
}
