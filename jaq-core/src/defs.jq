def empty: {}[] as $x | .;

def error:                error_empty  as $x | .    ;
def error(msgs):  (msgs | error_empty) as $x | .    ;

# Booleans
def true:  0 == 0;
def false: 0 != 0;
def not: if . then false else true end;
def select(f): if f then . else empty end;

# Conversion
def tostring: "\(.)";

# Generators
def range(from; to): range(from; to; 1);
def range(to): range(0; to);
def repeat(f): def rec: f, rec; rec;
def recurse(f): def rec: ., (f | rec); rec;
def recurse: recurse(.[]?);
def recurse(f; cond): recurse(f | select(cond));
def while(cond; update): def rec: if cond then ., (update | rec) else empty end; rec;
def until(cond; update): def rec: if cond then . else update | rec end; rec;

# Paths
def paths:    skip(1; path      (..));
def paths(p): skip(1; path_value(..)) | if .[1] | p then .[0] else empty end;
def getpath($path): reduce $path[] as $p (.; .[$p]);
def setpath($path; $x): getpath($path) = $x;

# Updates
def map(f): [.[] | f];
# Deleting multiple paths one at a time, in the order they were given, is only correct if
# an earlier deletion never shifts where a later one needs to land — false the moment two
# paths share an array prefix (`del(.[0,2])`: removing index 0 first turns the old index 2
# into the new index 1). Descending order avoids that: the highest index under any shared
# prefix always goes first, so nothing still to be deleted has moved by the time its turn
# comes (matches jq's own `delpaths`, verified against the oracle for both plain and
# nested multi-index deletes). Sorted by insertion (using `map`/`select`, defined just
# above) rather than a call to `sort`/`reverse`: those are jaq-std natives, unavailable to
# this (jaq-core) definition file — and this is defined here, not down with the rest of
# "Paths" above, because it needs `map` to already exist.
def delpaths($paths):
  reduce ($paths | reduce .[] as $p ([]; map(select(. > $p)) + [$p] + map(select(. <= $p))))[]
    as $path (.; getpath($path) |= empty);
def map_values(f): .[] |= f;
def walk(f): .. |= f;
# `f |= empty` (the previous definition) applies each of `f`'s paths against the
# *already-updated* value as it goes, hitting the same shift problem `delpaths` does —
# `del(.[0,2])` deleted old index 2 by that point, not old index 3. Collecting every path
# first with `path(f)`, then deleting them all via `delpaths` (already ordered to avoid
# the shift), matches jq's own definition.
def del(f): delpaths([path(f)]);

# Arrays
def first:  .[ 0];
def last:   .[-1];
def nth(n): .[ n];
def join($s): .[] |= tostring | .[:-1][] += $s | reduce .[] as $x (""; . + $x);
def combinations: .[][] |= [.] | reduce .[] as $a ([]; . + $a[]);
def combinations($n): [limit($n; repeat(.))] | combinations;

def nth($n; g): if $n < 0 then error("nth doesn't support negative indices") else first(skip($n; g)) end;

# Objects <-> Arrays
def   to_entries: [key_values[] as [$key, $value] | { $key, $value }];
def from_entries: reduce (.[] | { (.key): .value }) as $x ({}; . + $x);
def with_entries(f): to_entries | map(f) | from_entries;

# Predicates
def isempty(g): first((g | false), true);
def all(g; cond): isempty(g | cond and empty);
def any(g; cond): isempty(g | cond  or empty) | not;
def all(cond): all(.[]; cond);
def any(cond): any(.[]; cond);
def all: all(.[]; .);
def any: any(.[]; .);
