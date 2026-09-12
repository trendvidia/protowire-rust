# Bool map-key fixtures

Cross-port conformance fixtures for the spellings of a `map<bool, V>`
key (draft `-01` § Entries and Keys; issue #284, which measured four
ports binding four different sets of spellings).

All documents bind against [`bool-keys.proto`](bool-keys.proto) (package
`mapkeys.v1`): the bool-key documents against message `Flags`, one field
`map<bool, string> by_flag = 1`, and the string-key spelling pair against
message `Labels`, one field `map<string, string> by_label = 1`; each
file's `@type` directive names its message. As with
[`testdata/duration/`](../duration/), the harness wiring is per port;
`cmd/pxf/map_keys_test.go` runs every document here through the reference
implementation (#286, on protowire-go v1.6.0).

## MUST bind

| document | spelling | keys |
|---|---|---|
| [`bool-keys.pxf`](bool-keys.pxf) | quoted literal `"true"` / `"false"` | true, false |
| [`bool-keys-integer.pxf`](bool-keys-integer.pxf) | integer `1` / `0` ("bool encoded as 0/1") | true, false |
| [`bool-keys-keyword.pxf`](bool-keys-keyword.pxf) | keyword `true` / `false` | true, false |

The three documents decode to the same two keys with different values;
the spelling is not part of the value.

## MUST NOT bind

Every file under [`invalid/`](invalid/) is a document a conforming port
rejects with an error that names the key. None of them is a bool
literal:

- `identifier-*.pxf` — an identifier key (`TRUE`, `True`, `t`, `T`,
  `FALSE`, `False`, `f`, `F`, `yes`; the file names carry a case tag
  because the repository is checked out on case-insensitive filesystems
  too) on a bool K matches nothing. These
  are the spellings Go's `strconv.ParseBool` and Java's
  `Boolean.parseBoolean` admit; the grammar does not.
- `string-*.pxf` — a string key is parsed as a literal of K's type, and
  `"1"`, `"0"`, `"TRUE"`, `"t"` are not bool literals. `"1"` and `"0"` are
  the sharp edge: the integer spelling is valid bare and invalid quoted.

A port that binds any of these to a value has taken a language
convention where the grammar has two words (protowire-go#90, #93;
protowire-java#76).

## fmt canonicalization pairs (key spelling, issue #306)

Now that a bare `true` / `false` is a bool key and a bare `123` an
integer key, the quotes on a key are meaningful: `"true": "v"` on a
`map<string, V>` binds the string, `true: "v"` is an error. Draft `-01`
§ Entries and Keys ("Canonical spelling of map keys") therefore has a
formatter keep the document's spelling wherever changing it would change
what the key denotes: a bare key stays bare; a quoted key is unquoted
only when it is identifier-safe and not a value keyword. That needs the
parser to retain whether a map key was quoted, as it already does for
keyed entry names. Decided as option 2 on #306.

| Pair | Asserts |
|---|---|
| [`fmt-keyword-keys`](fmt-keyword-keys.pxf) | On a string-keyed map: `"true"`, `"false"`, `"null"` and `"123"` stay quoted; the quoted identifier-safe `"plain"` canonicalizes to bare; `bare` stays bare. The input also MUST bind, to six string keys. |
| [`fmt-bare-keys`](fmt-bare-keys.pxf) | On a bool-keyed map: a bare `true` and a bare `0` stay bare — a formatter does not add quotes the author did not write. A fixed point; the input also MUST bind, to the keys true and false. |
| [`fmt-dotted-keys`](fmt-dotted-keys.pxf) | On a string-keyed map: the *identifier* production admits `.`, so the quoted `"a.b"` canonicalizes to bare and a bare `c.d` stays bare — the same identifier-safe test keyed entry names already use (`user.name { }` is a legal unquoted entry name). `".e"` and `"1.5"` fail *ident-start* and stay quoted, so nothing float-shaped or leading-dot becomes bare. The marshaller writes `a.b:` bare under the same test. Both documents MUST bind, to four string keys. Issue #313, decided as (a): the ports' marshallers and formatters stopped at `[A-Za-z0-9_]` and wrote `"a.b":` where the text says bare. |

Both pairs are comment-free apart from `@type`, as in
[`testdata/keyed/`](../keyed/), so the byte-level expectation pins the
spelling and not comment placement. Note the second pair moves one
output every formatter in the family currently writes: a bare integer
key was quoted on the way through `fmt` (`404:` → `"404":`); it stays
bare now, as the marshaller has always written it.

The formatter-side wiring is per port (the reference's is
protowire-go#123); the marshaller side needs no change for the first two
pairs. The dotted pair moves both the formatter and the marshaller in
every port, sequenced after that port's #306 change so the two spelling
moves do not interleave (the reference's is protowire-go#125).

## Diagnostic

The rejection SHOULD name the offending key and the field, e.g.
`invalid bool map key t for field "by_flag": a bool key is true, false, 0,
1, "true" or "false"` (the reference wording since protowire-go v1.6.0,
which lists the keyword spelling too). Ports assert the verdict; the
wording is guidance.
