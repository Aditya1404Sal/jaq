
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
