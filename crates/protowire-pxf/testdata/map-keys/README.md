# Bool map-key fixtures

Cross-port conformance fixtures for the spellings of a `map<bool, V>`
key (draft `-01` § Entries and Keys; issue #284, which measured four
ports binding four different sets of spellings).

All documents bind against [`bool-keys.proto`](bool-keys.proto) (package
`mapkeys.v1`, message `Flags`, one field `map<bool, string> by_flag = 1`);
each file's `@type` directive names the message. As with
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

## Diagnostic

The rejection SHOULD name the offending key and the field, e.g.
`invalid bool map key t for field "by_flag": a bool key is true, false, 0,
1, "true" or "false"` (the reference wording since protowire-go v1.6.0,
which lists the keyword spelling too). Ports assert the verdict; the
wording is guidance.
