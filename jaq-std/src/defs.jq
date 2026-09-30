def null:  [][0];

def stderr:      (       stderr_empty  as $x | .), .;
def debug:       (        debug_empty  as $x | .), .;
def debug(msgs): ((msgs | debug_empty) as $x | .), .;

def halt: halt(0);
def halt_error($exit_code): stderr_empty, halt($exit_code);
def halt_error: halt_error(5);

# Not defined in jq!
def isboolean: . == true or . == false;
def isnumber:  . > true and . < "";
def isstring:  . >= ""  and . < [];
def isarray:   . >= []  and . < {};
def isobject:  . >= {};

# Numbers
# `nan`/`infinite` are native filters (jaq-std/src/lib.rs), not jq-level definitions: they used
# to be `0/0`/`1/0`, but `/` now raises jq's own zero-divisor error instead of letting it through
# to the IEEE result, so getting an actual NaN/Infinity value needs a real primitive.
def isnan:      . < nan and nan < .;
def isinfinite: . == infinite or  . == -infinite;
def isfinite:   isnumber and (isinfinite | not);
def isnormal:   isnumber and ((. == 0 or isnan or isinfinite) | not);

# Math
def abs: if . < 0 then - . end;
def logb:
    if . == 0.0 then -infinite
  elif isinfinite then infinite
  elif isnan then .
  else ilogb | . + 0.0 end;
def significand:
    if isinfinite or isnan then .
  elif . == 0.0 then 0.0
  else scalbln(.; ilogb | -1 * .) end;
def pow10:            pow(10.0; .);
def drem($l; r):      remainder($l; r) | if . == 0 then copysign(.; $l) end;
def nexttoward(x; y): nextafter(x; y);
def scalb(x; e):      x * pow(2.0; e);
def gamma: tgamma;

# Type
def type:
    if . == null then "null"
  elif isboolean then "boolean"
  elif . < "" then "number"
  elif . < [] then "string"
  elif . < {} then "array"
  else             "object" end;

# Selection
def values:    select(. != null);
def nulls:     select(. == null);
def booleans:  select(isboolean);
def numbers:   select(isnumber);
def finites:   select(isfinite);
def normals:   select(isnormal);
def strings:   select(isstring);
def arrays:    select(isarray);
def objects:   select(isobject);
def iterables: select(. >= []);
def scalars:   select(. <  []);

# Iterators
def add(f): reduce f as $x (null; . + $x);
def add: add(.[]);

# Arrays
def min_by(f): reduce min_by_or_empty(f) as $x (null; $x);
def max_by(f): reduce max_by_or_empty(f) as $x (null; $x);
def min: reduce min_or_empty as $x (null; $x);
def max: reduce max_or_empty as $x (null; $x);
def unique_by(f): [group_by(f)[] | .[0]];
def unique: sort | unique_by(.);

# Paths
def pick(f):
  reduce path_value(f) as [$path, $value] ({}; . *
    reduce ($path | reverse[]) as $p ($value; {($p): .})
  );

def keys: keys_unsorted | sort;

def _flatten($x): reduce .[] as $i ([];
  if $i | type == "array" and $x != 0 then . + ($i | _flatten($x - 1)) else . + [$i] end);
def flatten($x): if $x < 0 then error("flatten depth must not be negative") else _flatten($x) end;
def flatten: _flatten(-1);

# Regular expressions
def capture_of_match: map(select(.name) | { (.name): .string} ) | add + {};

def    test(re; flags): matches(re; flags) | any;
# `scan` always finds every match, the same way `gsub` above always finds every match to
# substitute — regardless of whether the caller's own `flags` argument happens to include
# `g` for "global" (verified against the oracle: `scan(re; "")` finds all matches, not
# just the first, same as `scan(re)`).
def   match(re; flags): matches(re; flags)[] | .[0] + { captures: .[1:] };
def    scan(re; flags): match(re; "g" + flags) |
  if .captures != [] then [.captures[].string] else .string end;
def capture(re; flags): matches(re; flags)[] | capture_of_match;

def split($sep):
  if isstring and ($sep | isstring) then . / $sep
  else error("split input and separator must be strings") end;
def split (re; flags): split_(re; flags + "g");
def splits(re; flags): split(re; flags)[];

def sub(re; f; flags):
  def handle: if isarray then capture_of_match | f end;
  reduce split_matches(re; flags)[] as $x (""; . + ($x | handle));

def gsub(re; f; flags): sub(re; f; "g" + flags);

def test($val): ($val | type) as $vt | if $vt == "string" then test($val; null)
  elif $vt == "array" and $val != [] then test($val[0]; $val[1])
  else error($vt + " not a string or array") end;
def    scan(re):    scan(re; null);
def match($val): ($val | type) as $vt | if $vt == "string" then match($val; null)
  elif $vt == "array" and $val != [] then match($val[0]; $val[1])
  else error($vt + " not a string or array") end;
def capture($val): ($val | type) as $vt | if $vt == "string" then capture($val; null)
  elif $vt == "array" and $val != [] then capture($val[0]; $val[1])
  else error($vt + " not a string or array") end;
def  splits(re):  splits(re; "");
def  sub(re; f): sub(re; f;  "");
def gsub(re; f): sub(re; f; "g");

# Date
# jq 1.8's ISO 8601 conversions: whole seconds only
def fromdateiso8601: strptime("%Y-%m-%dT%H:%M:%SZ") | mktime;
def todateiso8601: strftime("%Y-%m-%dT%H:%M:%SZ");
def   todate:   todateiso8601;
def fromdate: fromdateiso8601;

# Formatting
def @sh: [if isarray then .[] end | if . >= "" then "'\(escape_sh)'" else "\(.)" end] | join(" ");
def @text: "\(.)";
def @html   : tostring | escape_html;
def @htmld  : tostring | unescape_html;
def @uri    : tostring | encode_uri;
def @urid   : tostring | decode_uri;
def @base64 : tostring | encode_base64;
def @base64d: tostring | decode_base64;

# jq's SQL-style operators
def INDEX(stream; idx_expr): reduce stream as $row ({}; .[$row | idx_expr | tostring] = $row);
def INDEX(idx_expr): INDEX(.[]; idx_expr);
def JOIN($idx; idx_expr): [.[] | [., $idx[idx_expr]]];
def JOIN($idx; stream; idx_expr): stream | [., $idx[idx_expr]];
def JOIN($idx; stream; idx_expr; join_expr): stream | [., $idx[idx_expr]] | join_expr;
def IN(s): any(s == .; .);
def IN(src; s): any(src == s; .);

def trimstr($val): ltrimstr($val) | rtrimstr($val);
