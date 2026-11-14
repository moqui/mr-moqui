# The built-in pipeline of a Moqui component (docs/interfaces/Pipelines.md "The built-in pipeline"): the component's
# regression suite, in a runtime with the framework and the components it depends on. The image holds an MR Moqui
# tree at MOQUI_HOME (moq, addons.xml, runtime and a built moqui binary), git and bash.
set -eu
MOQUI_HOME="${MOQUI_HOME:-/opt/moqui}"
name="${MOQUI_CI_COMPONENT:?the component name}"
src="$(pwd)"
if [ ! -x "$MOQUI_HOME/moq" ]; then
    echo "this image has no MR Moqui tree at $MOQUI_HOME: the built-in pipeline needs moq, addons.xml and a built moqui binary there" >&2
    exit 2
fi
bin="$MOQUI_HOME/target/release/moqui"
[ -x "$bin" ] || bin="$MOQUI_HOME/target/debug/moqui"
if [ ! -x "$bin" ]; then
    echo "this image has no built moqui binary under $MOQUI_HOME/target" >&2
    exit 2
fi
have="$("$bin" version | sed -n 's/^moqui \([^ ]*\).*/\1/p')"
want="${MOQUI_CI_FRAMEWORK_VERSION:-}"
if [ -n "$want" ] && [ "$want" != "$have" ]; then
    echo "component $name names framework version $want and this image has $have: run it in an image of that version" >&2
    exit 2
fi
echo "MR Moqui $have, component $name"
# a job's container has a read-only root: the tree is copied beside the checkout, where it can be written
if [ ! -w "$MOQUI_HOME/runtime" ]; then
    work="$(dirname "$src")/moqui-tree"
    rm -rf "$work"
    mkdir -p "$work"
    tar -C "$MOQUI_HOME" -cf - . | tar -C "$work" -xf -
    MOQUI_HOME="$work"
fi
dest="$MOQUI_HOME/runtime/components/addon/$name"
rm -rf "$dest"
mkdir -p "$dest"
tar -C "$src" --exclude=.git --exclude=test-results -cf - . | tar -C "$dest" -xf -
cd "$MOQUI_HOME"
./moq component-get "$name"
mkdir -p "$src/test-results"
./moq test "$name" --nobuild --junit "$src/test-results" --workers "${MOQUI_CI_WORKERS:-2}"
