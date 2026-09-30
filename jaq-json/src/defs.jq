
# Arrays
def transpose: [range([.[] | length] | max) as $i | [.[][$i]]];

# Indexing
def in(xs)    : . as $x | xs | has     ($x);
def inside(xs): . as $x | xs | contains($x);
def  index($i): indices($i)[ 0];
def rindex($i): indices($i)[-1];

# Formatting
def @json: tojson;

# Numbers keep the text they were read from, as jq's decNumber literals do.
def have_decnum: true;
def have_literal_numbers: true;

# jq's streaming forms, as jq 1.8 defines them
def tostream:
  path(def r: (.[]? | r), .; r) as $p |
  getpath($p) |
  reduce path(.[]?) as $q ([$p, .]; [$p + $q]);
def fromstream(i): {x: null, e: false} as $init |
  foreach i as $i ($init;
    if .e then $init else . end |
    if $i | length == 2
    then setpath(["e"]; $i[0] | length == 0) | setpath(["x"] + $i[0]; $i[1])
    else setpath(["e"]; $i[0] | length == 1) end;
    if .e then .x else empty end);
def truncate_stream(stream):
  . as $n | null | stream | . as $input |
  if (.[0] | length) > $n then setpath([0]; $input[0][$n:]) else empty end;

# jq 1.8's object builders: a key that is not a string is an error
def from_entries: map({ (.key // .Key // .name // .Name):
  if has("value") then .value else .Value end }) | add // {};
def with_entries(f): to_entries | map(f) | from_entries;

# jq 1.8's `reverse`: through indices, so a string or number errs and `null` gives `[]`
def reverse: [.[length - 1 - range(0; length)]];

# jq 1.8's `combinations`, through indices
def combinations: if length == 0 then [] else .[0][] as $x | (.[1:] | combinations) as $y | [$x] + $y end;
def combinations(n): . as $dot | [range(n) | $dot] | combinations;
