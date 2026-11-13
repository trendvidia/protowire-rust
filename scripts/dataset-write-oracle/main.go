// Copyright 2026 TrendVidia LLC
// SPDX-License-Identifier: MIT

// dataset-write-oracle writes what protowire-go's @dataset writer
// (AppendDatasetHeader, AppendDatasetRow) does with each case of
// dataset-write-cases.tsv (#129), and what its reader reads back from the
// table it wrote. It builds inside a protowire-go checkout, so run it through
// scripts/gen-dataset-write-oracle.sh.
//
//	dataset-write-oracle <testdata dir> <cases.tsv>
//
// A case is: descriptor set, message, columns, build. The columns are
// comma-separated, "-" for none, or "mask:<n>" for the columns of dsw.v1.Row
// whose bit is set in n, with its defaulted columns always among them (Go's
// FuzzDatasetWriteRoundTrip). The build makes the message, from a new one, in
// steps joined by " ;; ":
//
//	header           no message: the case is the header alone
//	new              the new message, unchanged
//	unmarshal <doc>  Unmarshal doc into the message
//	set <doc>        the same with SkipPostDecode, so no default is applied
//	                 and no required field checked: what Go's tests do by
//	                 setting fields through protoreflect
//	full <doc>       UnmarshalFull doc into the message
//	mask <path>      append path to the message's _null FieldMask
//	clear <field>    clear the field
//	random <seed> <n>         n random rows of dsw.v1.Row, as Go's
//	                          TestDatasetWrite_RoundTrip makes them
//	random-subset <seed> <n>  the same, with oneof members set now and then,
//	                          as Go's FuzzDatasetWriteRoundTrip makes them
//	random-dotted <seed> <n>  n rows for dotted columns under inner, as Go's
//	                          TestDatasetWrite_DottedRoundTrip makes them
//
// Each row is the case with its columns resolved, the row's index among the
// case's messages, the message's pb hex, then the verdicts, each OK and a
// text or ERR and the error: the header, the row, the message the reader
// reads back from header and row (its pb hex, or "=" when that is the
// message's own; "-" when either failed), and
// Marshal of the message ("-" for random rows). Scan and BindRow read the
// table back, and the oracle stops if they disagree. Tabs, newlines and
// backslashes in a verdict are escaped.
package main

import (
	"bufio"
	"bytes"
	"encoding/hex"
	"fmt"
	"math"
	"math/rand/v2"
	"os"
	"path/filepath"
	"slices"
	"strconv"
	"strings"

	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/reflect/protodesc"
	"google.golang.org/protobuf/reflect/protoreflect"
	"google.golang.org/protobuf/reflect/protoregistry"
	"google.golang.org/protobuf/types/descriptorpb"
	"google.golang.org/protobuf/types/dynamicpb"

	"github.com/trendvidia/protowire-go/encoding/pxf"
)

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

func escape(s string) string {
	return strings.NewReplacer(`\`, `\\`, "\t", `\t`, "\n", `\n`).Replace(s)
}

func verdict(text []byte, err error) string {
	if err != nil {
		return "ERR\t" + escape(err.Error())
	}
	return "OK\t" + escape(string(text))
}

func pbHex(m proto.Message) string {
	b, err := proto.MarshalOptions{Deterministic: true}.Marshal(m)
	check(err)
	return hex.EncodeToString(b)
}

// The column lists and generators of protowire-go's dataset_write_test.go,
// copied: the oracle's random rows are the ones its tests write.

var rowColumns = []string{
	"s", "b", "i32", "si32", "sf32", "i64", "si64", "sf64", "u32", "f32", "u64", "f64",
	"fl", "db", "by", "color", "opt",
	"ts", "dur", "w_s", "w_i64", "w_b", "w_by", "bi", "dec", "bf", "sec",
	"d_s", "d_i32", "d_opt", "d_w", "d_ts", "d_color",
	"c_w", "c_i", "c_b", "o_s", "o_i",
	"list", "m", "inner", "inners", "keyed", "by_key",
}

var defaultedColumns = []string{"d_s", "d_i32", "d_opt", "d_w", "d_ts", "d_color", "c_w", "o_s"}

func makeWritable(m *dynamicpb.Message, cols []string) {
	desc := m.Descriptor()
	if fd := desc.Fields().ByName("d_opt"); slices.Contains(cols, "d_opt") && !m.Has(fd) {
		m.Set(fd, protoreflect.ValueOfInt32(0))
	}
	set := m.WhichOneof(desc.Oneofs().ByName("other"))
	if slices.Contains(cols, "o_s") && (set == nil || !slices.Contains(cols, string(set.Name()))) {
		m.Set(desc.Fields().ByName("o_s"), protoreflect.ValueOfString(""))
	}
}

func randomRow(r *rand.Rand, desc protoreflect.MessageDescriptor) *dynamicpb.Message {
	m := dynamicpb.NewMessage(desc)
	for _, c := range rowColumns {
		if r.IntN(3) == 0 {
			continue
		}
		fd := desc.Fields().ByName(protoreflect.Name(c))
		if doc := randomNested(r, c); doc != "" {
			check(pxf.Unmarshal([]byte(doc), m))
			continue
		}
		if fd.Kind() == protoreflect.MessageKind {
			lit := randomLiteral(r, fd.Message().FullName())
			check(pxf.Unmarshal([]byte(c+" = "+lit), m))
			continue
		}
		m.Set(fd, randomScalar(r, fd))
	}
	makeWritable(m, rowColumns)
	return m
}

var trickyStrings = []string{"", "plain", "a, b", "(x)", "\"q\"", "back\\slash", "line\nbreak", "tab\t", "# not a comment", "// nor this", "ünï ✓", "null", "true", "@dataset"}

func randomNested(r *rand.Rand, col string) string {
	inner := func() string {
		switch r.IntN(3) {
		case 0:
			return "{}"
		case 1:
			return fmt.Sprintf("{ x = %d }", r.Int32()-r.Int32())
		}
		return fmt.Sprintf("{ x = %d label = %q }", r.IntN(9), trickyStrings[r.IntN(len(trickyStrings))])
	}
	n := r.IntN(4)
	var parts []string
	switch col {
	case "list":
		for range n {
			parts = append(parts, fmt.Sprint(r.Int32()-r.Int32()))
		}
		return "list = [" + strings.Join(parts, ", ") + "]"
	case "m":
		for i := range n {
			parts = append(parts, fmt.Sprintf("%q: %d", fmt.Sprint(i)+trickyStrings[r.IntN(len(trickyStrings))], r.IntN(100)))
		}
		return "m = { " + strings.Join(parts, " ") + " }"
	case "inner":
		return "inner " + inner()
	case "inners":
		for range n {
			parts = append(parts, inner())
		}
		return "inners = [" + strings.Join(parts, ", ") + "]"
	case "keyed":
		if r.IntN(2) == 0 {
			for i := range n {
				parts = append(parts, fmt.Sprintf("k%d { v = %d }", i, r.IntN(9)))
			}
			return "keyed { " + strings.Join(parts, " ") + " }"
		}
		for range n {
			parts = append(parts, fmt.Sprintf("{ v = %d }", r.IntN(9)))
		}
		return "keyed = [" + strings.Join(parts, ", ") + "]"
	case "by_key":
		for i := range n {
			parts = append(parts, fmt.Sprintf("%q: %s", fmt.Sprint("k", i), inner()))
		}
		return "by_key = { " + strings.Join(parts, " ") + " }"
	}
	return ""
}

func randomScalar(r *rand.Rand, fd protoreflect.FieldDescriptor) protoreflect.Value {
	switch fd.Kind() {
	case protoreflect.StringKind:
		return protoreflect.ValueOfString(trickyStrings[r.IntN(len(trickyStrings))])
	case protoreflect.BoolKind:
		return protoreflect.ValueOfBool(r.IntN(2) == 0)
	case protoreflect.Int32Kind, protoreflect.Sint32Kind, protoreflect.Sfixed32Kind:
		return protoreflect.ValueOfInt32([]int32{0, -1, 1, math.MinInt32, math.MaxInt32, r.Int32()}[r.IntN(6)])
	case protoreflect.Int64Kind, protoreflect.Sint64Kind, protoreflect.Sfixed64Kind:
		return protoreflect.ValueOfInt64([]int64{0, -1, math.MinInt64, math.MaxInt64, r.Int64() - r.Int64()}[r.IntN(5)])
	case protoreflect.Uint32Kind, protoreflect.Fixed32Kind:
		return protoreflect.ValueOfUint32([]uint32{0, 1, math.MaxUint32, r.Uint32()}[r.IntN(4)])
	case protoreflect.Uint64Kind, protoreflect.Fixed64Kind:
		return protoreflect.ValueOfUint64([]uint64{0, 1, math.MaxUint64, r.Uint64()}[r.IntN(4)])
	case protoreflect.FloatKind:
		return protoreflect.ValueOfFloat32([]float32{0, -0.5, 3.25, math.MaxFloat32, math.SmallestNonzeroFloat32, float32(math.Inf(-1)), float32(math.NaN()), r.Float32()}[r.IntN(8)])
	case protoreflect.DoubleKind:
		return protoreflect.ValueOfFloat64([]float64{0, -1e300, 1.0 / 3, math.MaxFloat64, math.SmallestNonzeroFloat64, math.Inf(1), math.NaN(), r.NormFloat64()}[r.IntN(8)])
	case protoreflect.BytesKind:
		b := make([]byte, r.IntN(12))
		for i := range b {
			b[i] = byte(r.Uint32())
		}
		return protoreflect.ValueOfBytes(b)
	case protoreflect.EnumKind:
		return protoreflect.ValueOfEnum(protoreflect.EnumNumber(r.IntN(3)))
	}
	panic(fmt.Sprintf("no generator for %s", fd.Kind()))
}

func randomLiteral(r *rand.Rand, name protoreflect.FullName) string {
	switch name {
	case "google.protobuf.Timestamp":
		return []string{"1970-01-01T00:00:00Z", "2026-10-23T12:34:56.789Z", "0001-01-01T00:00:00Z", "9999-12-31T23:59:59.999999999Z"}[r.IntN(4)]
	case "google.protobuf.Duration":
		return []string{"0s", "1.5s", "-3h2m", "1ns", "-1µs"}[r.IntN(5)]
	case "google.protobuf.StringValue", "pxf.Secret":
		return fmt.Sprintf("%q", trickyStrings[r.IntN(len(trickyStrings))])
	case "google.protobuf.Int64Value":
		return []string{"0", "-9223372036854775808", "42"}[r.IntN(3)]
	case "google.protobuf.Int32Value":
		return []string{"0", "-2147483648", "42"}[r.IntN(3)]
	case "google.protobuf.BoolValue":
		return []string{"true", "false"}[r.IntN(2)]
	case "google.protobuf.BytesValue":
		return []string{`b""`, `b"AQID"`}[r.IntN(2)]
	case "pxf.BigInt":
		return []string{"0", "-1", "123456789012345678901234567890"}[r.IntN(3)]
	case "pxf.Decimal":
		return []string{"0", "-0.001", "3.14159265358979323846", "100000000000000000000"}[r.IntN(4)]
	case "pxf.BigFloat":
		return []string{"0", "-2.5", "1.25e-30"}[r.IntN(3)]
	}
	panic(fmt.Sprintf("no literal generator for %s", name))
}

// randomDotted is one row of Go's TestDatasetWrite_DottedRoundTrip.
func randomDotted(r *rand.Rand, desc protoreflect.MessageDescriptor, cols []string) *dynamicpb.Message {
	m := dynamicpb.NewMessage(desc)
	var parts []string
	if r.IntN(2) == 0 {
		parts = append(parts, fmt.Sprintf("s = %q", trickyStrings[r.IntN(len(trickyStrings))]))
	}
	if r.IntN(4) > 0 {
		var in []string
		if r.IntN(2) == 0 {
			in = append(in, fmt.Sprintf("x = %d", r.IntN(5)))
		}
		if r.IntN(2) == 0 {
			in = append(in, fmt.Sprintf("label = %q", trickyStrings[r.IntN(len(trickyStrings))]))
		}
		if r.IntN(2) == 0 {
			in = append(in, fmt.Sprintf("deep { %s }", []string{"", "n = 0", "n = 4"}[r.IntN(3)]))
		}
		parts = append(parts, "inner { "+strings.Join(in, " ")+" }")
	}
	check(pxf.Unmarshal([]byte(strings.Join(parts, "\n")), m))
	makeWritable(m, cols)
	return m
}

// columns resolves a case's column spec.
func columns(spec string) []string {
	if spec == "-" {
		return nil
	}
	if n, ok := strings.CutPrefix(spec, "mask:"); ok {
		mask, err := strconv.ParseUint(n, 10, 64)
		check(err)
		var cols []string
		for i, c := range rowColumns {
			if mask&(1<<uint(i)) != 0 || slices.Contains(defaultedColumns, c) {
				cols = append(cols, c)
			}
		}
		return cols
	}
	return strings.Split(spec, ",")
}

// build makes the case's messages; nil for a header case.
func build(desc protoreflect.MessageDescriptor, cols []string, steps string) []*dynamicpb.Message {
	if steps == "header" {
		return nil
	}
	op, arg, _ := strings.Cut(steps, " ")
	if op == "random-dotted" {
		f := strings.Fields(arg)
		seed, err := strconv.ParseUint(f[0], 10, 64)
		check(err)
		n, err := strconv.Atoi(f[1])
		check(err)
		r := rand.New(rand.NewPCG(seed, 155))
		out := make([]*dynamicpb.Message, n)
		for i := range out {
			out[i] = randomDotted(r, desc, cols)
		}
		return out
	}
	if op == "random" || op == "random-subset" {
		f := strings.Fields(arg)
		seed, err := strconv.ParseUint(f[0], 10, 64)
		check(err)
		n, err := strconv.Atoi(f[1])
		check(err)
		if op == "random" {
			r := rand.New(rand.NewPCG(seed, 1))
			out := make([]*dynamicpb.Message, n)
			for i := range out {
				out[i] = randomRow(r, desc)
			}
			return out
		}
		r := rand.New(rand.NewPCG(seed, 7))
		out := make([]*dynamicpb.Message, n)
		for i := range out {
			out[i] = randomRow(r, desc)
			for _, c := range []string{"c_i", "c_b", "o_i"} {
				if r.IntN(4) == 0 {
					fd := desc.Fields().ByName(protoreflect.Name(c))
					out[i].Set(fd, randomScalar(r, fd))
				}
			}
			makeWritable(out[i], cols)
		}
		return out
	}
	m := dynamicpb.NewMessage(desc)
	for _, step := range strings.Split(steps, " ;; ") {
		op, arg, _ := strings.Cut(step, " ")
		switch op {
		case "new":
		case "unmarshal":
			check(pxf.Unmarshal([]byte(arg), m))
		case "set":
			check(pxf.UnmarshalOptions{SkipPostDecode: true}.Unmarshal([]byte(arg), m))
		case "full":
			_, err := pxf.UnmarshalFull([]byte(arg), m)
			check(err)
		case "mask":
			mask := m.Mutable(desc.Fields().ByName("_null")).Message()
			paths := mask.Mutable(mask.Descriptor().Fields().ByName("paths")).List()
			paths.Append(protoreflect.ValueOfString(arg))
		case "clear":
			m.Clear(desc.Fields().ByName(protoreflect.Name(arg)))
		default:
			check(fmt.Errorf("unknown step %q", step))
		}
	}
	return []*dynamicpb.Message{m}
}

// readBack is the one row the reader reads from table, through Scan, and
// the same row through Parse and BindRow, which must agree.
func readBack(desc protoreflect.MessageDescriptor, table []byte) string {
	scanned := dynamicpb.NewMessage(desc)
	scanErr := func() error {
		tr, err := pxf.NewDatasetReader(bytes.NewReader(table))
		if err != nil {
			return err
		}
		return tr.Scan(scanned)
	}()
	bound := dynamicpb.NewMessage(desc)
	bindErr := func() error {
		doc, err := pxf.Parse(table)
		if err != nil {
			return err
		}
		ds := doc.Datasets[0]
		return pxf.BindRow(bound, ds.Columns, ds.Rows[0])
	}()
	if fmt.Sprint(scanErr) != fmt.Sprint(bindErr) || (scanErr == nil && !proto.Equal(scanned, bound)) {
		check(fmt.Errorf("Scan and BindRow disagree on %q: %v / %v", table, scanErr, bindErr))
	}
	if scanErr != nil {
		return verdict(nil, scanErr)
	}
	return "OK\t" + pbHex(scanned)
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
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}
		c := strings.SplitN(line, "\t", 4)
		files := loaded[c[0]]
		if files == nil {
			files = load(filepath.Join(dir, c[0]))
			loaded[c[0]] = files
		}
		d, err := files.FindDescriptorByName(protoreflect.FullName(c[1]))
		check(err)
		desc := d.(protoreflect.MessageDescriptor)
		cols := columns(c[2])
		resolved := strings.Join(cols, ",")
		if cols == nil {
			resolved = "-"
		}
		head, headErr := pxf.AppendDatasetHeader(nil, desc, cols)
		msgs := build(desc, cols, c[3])
		if msgs == nil {
			fmt.Fprintf(out, "%s\t%s\t%s\t%s\t-\t-\t%s\t-\t-\t-\t-\t-\t-\n", c[0], c[1], resolved, c[3], verdict(head, headErr))
			continue
		}
		random := strings.HasPrefix(c[3], "random")
		for i, m := range msgs {
			row, rowErr := pxf.AppendDatasetRow(nil, m, cols)
			back := "-\t-"
			if headErr == nil && rowErr == nil {
				back = readBack(desc, append(slices.Clone(head), row...))
			}
			msgHex := pbHex(m)
			if back == "OK\t"+msgHex {
				back = "OK\t="
			}
			marshal := "-\t-"
			if !random {
				text, err := pxf.Marshal(m)
				marshal = verdict(text, err)
			}
			fmt.Fprintf(out, "%s\t%s\t%s\t%s\t%d\t%s\t%s\t%s\t%s\t%s\n", c[0], c[1], resolved, c[3], i, msgHex, verdict(head, headErr), verdict(row, rowErr), back, marshal)
		}
	}
	check(sc.Err())
}
