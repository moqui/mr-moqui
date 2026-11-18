# shellcheck shell=bash
# moq's component-get, -depends, -create and -zip, addons.xml and myaddons.xml. Sourced by moq, which holds the task table; bash 3.2, as moq is.

# ========== components (addons.xml, myaddons.xml, component.xml) ==========

# The file without <!-- --> comments (including ones spanning lines), and with a tag that spans lines joined onto one.
xml_strip_comments() {
    [ -f "$1" ] || return 0
    awk '
        function last(s, c,    i, n) { n = 0; for (i = 1; i <= length(s); i++) if (substr(s, i, 1) == c) n = i; return n }
        {
            line = $0; out = ""
            while (1) {
                if (incomment) {
                    i = index(line, "-->")
                    if (i == 0) { line = ""; break }
                    line = substr(line, i + 3); incomment = 0
                }
                i = index(line, "<!--")
                if (i == 0) { out = out line; break }
                out = out substr(line, 1, i - 1); line = substr(line, i + 4); incomment = 1
            }
            buf = (pending ? buf " " out : out)
            pending = (last(buf, "<") > last(buf, ">"))
            if (!pending) print buf
        }
        END { if (pending) print buf }' "$1"
}

# Lines of file $1 holding element $2. These files keep one element per line, once tags are joined.
xml_elements() {
    xml_strip_comments "$1" | grep "<$2[[:space:]/>]"
}

xml_attr() {
    printf '%s\n' "$1" | sed -n "s/.*[[:space:]]$2=\"\\([^\"]*\\)\".*/\\1/p" | head -n 1
}

# The element of type $1 with name $2, from myaddons.xml first, then the served addons.xml files, then addons.xml.
addons_find() {
    local file line
    for file in "${ADDONS_FILES[@]}"; do
        while IFS= read -r line; do
            if [ "$(xml_attr "$line" name)" = "$2" ]; then
                printf '%s\n' "$line"
                return 0
            fi
        done < <(xml_elements "$file" "$1")
    done
    return 1
}

addons_names() {
    local file line
    for file in "${ADDONS_FILES[@]}"; do
        while IFS= read -r line; do
            xml_attr "$line" name
        done < <(xml_elements "$file" "$1")
    done | sort -u
}

addons_root_attr() {
    local line
    line=$(xml_elements "$1" addons | head -n 1)
    [ -n "$line" ] && xml_attr "$line" "$2"
}

# The url template of location type $2 in repository $1, from myaddons.xml first, then the served addons.xml files,
# then addons.xml.
addons_location() {
    local file url
    for file in "${ADDONS_FILES[@]}"; do
        url=$(xml_strip_comments "$file" | awk -v repo="$1" -v type="$2" '
            /<repository[[:space:]]/ {
                inrepo = (match($0, "[[:space:]]name=\"" repo "\"") > 0)
            }
            inrepo && /<location[[:space:]]/ && match($0, "[[:space:]]type=\"" type "\"") {
                if (match($0, /url="[^"]*"/)) { print substr($0, RSTART + 5, RLENGTH - 6); exit }
            }
            /<\/repository>/ { inrepo = 0 }')
        if [ -n "$url" ]; then
            printf '%s\n' "$url"
            return 0
        fi
        # A repository found in myaddons.xml without this type does not fall through to addons.xml; one that only names
        # a server's addons.xml (type addons) does, to that file's repository of the same name.
        if xml_strip_comments "$file" | awk -v repo="$1" '
            /<repository[[:space:]]/ { inrepo = (match($0, "[[:space:]]name=\"" repo "\"") > 0); if (inrepo && /<repository[^>]*\/>/) found = 1 }
            inrepo && /<location[[:space:]]/ && !/[[:space:]]type="addons"/ { found = 1 }
            /<\/repository>/ { inrepo = 0 }
            END { exit !found }'; then
            return 1
        fi
    done
    return 1
}

# Fill ${group} style placeholders, and the Groovy ${component.'@group'} ones a Java myaddons.xml uses, from element $2.
expand_url() {
    local url=$1 element=$2 attr value
    for attr in group name branch version token; do
        value=$(xml_attr "$element" "$attr")
        url=${url//"\${$attr}"/"$value"}
        url=${url//"\${component.'@$attr'}"/"$value"}
    done
    printf '%s\n' "$url"
}

default_location_type() {
    if [ -e "$ROOT/.git" ]; then echo git; else echo current; fi
}

# Set by parse_component_args from -r; empty means fall back to the element and the defaults.
REPOSITORY=

# The default repository when neither -r, the component element, nor default-repository says one: github for an
# https clone of this repo, github-ssh for a git@ or ssh:// one, and github when there is no origin to ask.
DEFAULT_REPOSITORY=
default_repository() {
    if [ -z "$DEFAULT_REPOSITORY" ]; then
        local url=
        if command -v git >/dev/null 2>&1; then
            url=$(git -C "$ROOT" remote get-url origin 2>/dev/null) || url=
        fi
        case $url in
            git@* | ssh://*) DEFAULT_REPOSITORY=github-ssh ;;
            *) DEFAULT_REPOSITORY=github ;;
        esac
    fi
    printf '%s\n' "$DEFAULT_REPOSITORY"
}

check_location_type() {
    case $1 in
        git | current | release | binary) ;;
        *) die "location type '$1' is not git, current, release, or binary" ;;
    esac
}

# Download component $1 (addons element $3) by type $2 into $4.
download_component() {
    local name=$1 type=$2 element=$3 target=$4 version branch repository url work top
    version=$(xml_attr "$element" version)
    branch=$(xml_attr "$element" branch)
    case $type in release | binary) [ -n "$version" ] || type=current ;; esac
    # -r wins, then the element's repository attribute, then default-repository, then the detected default.
    if [ -n "$REPOSITORY" ]; then
        repository=$REPOSITORY
    else
        repository=$(xml_attr "$element" repository)
        [ -n "$repository" ] || repository=$(addons_root_attr "$MYADDONS" default-repository)
        [ -n "$repository" ] || repository=$(addons_root_attr "$ADDONS" default-repository)
        [ -n "$repository" ] || repository=$(default_repository)
    fi
    url=$(addons_location "$repository" "$type") || die "repository $repository has no $type location in myaddons.xml or addons.xml"
    url=$(expand_url "$url" "$element")
    info "Getting $name ($type) from $url${branch:+ at $branch} to $(rel "$target")"
    if [ "$type" = git ]; then
        need_tool git
        if [ -n "$branch" ]; then
            # `--` before the URL: a myaddons.xml URL that starts with `-` is not an option of git's
            run_cmd git clone -b "$branch" -- "$url" "$target" || die "git clone of $name failed"
        else
            run_cmd git clone -- "$url" "$target" || die "git clone of $name failed"
        fi
        return 0
    fi
    need_tool curl unzip
    if dry_run; then
        show_cmd curl -fL -o "$target.zip" "$url"
        show_cmd unzip -q "$target.zip" -d "<temp>"
        show_cmd mv "<temp>/<top>" "$target"
        return 0
    fi
    curl -fL --progress-bar -o "$target.zip" "$url" || {
        rm -f "$target.zip"
        die "download of $name failed"
    }
    # a version's archive is fixed, so it is checked against its first download; a branch's moves
    case $type in release | binary) check_download_sum "$target.zip" "addon:$url" ;; esac
    work=$(mktemp -d "$(dirname "$target")/.moq-get.XXXXXX") || die "could not make a temp directory"
    if ! unzip -q "$target.zip" -d "$work"; then
        rm -rf "$work" "$target.zip"
        die "could not unpack the $name archive"
    fi
    rm -f "$target.zip"
    # Archives hold one top directory (name-branch or name-version); move its contents up a level as Gradle did.
    top=$(ls -A "$work")
    if [ -n "$top" ] && [ "$(printf '%s\n' "$top" | wc -l)" -eq 1 ] && [ -d "$work/$top" ]; then
        mv "$work/$top" "$target" && rmdir "$work"
    else
        mv "$work" "$target"
    fi || die "could not move $name into $(rel "$target")"
    info "Downloaded $name to $(rel "$target")"
}

# Component names already checked in this run, space separated.
COMPONENTS_CHECKED=' '

# The myaddons.xml or addons.xml component that $1 names: the one of that name, else the one whose component-name is $1,
# a fork whose repository and directory are named apart from its component.xml name (my-shop is shop).
addons_name_for() {
    local file line
    if addons_find component "$1" >/dev/null; then
        printf '%s\n' "$1"
        return 0
    fi
    for file in "${ADDONS_FILES[@]}"; do
        while IFS= read -r line; do
            if [ "$(xml_attr "$line" component-name)" = "$1" ]; then
                xml_attr "$line" name
                return 0
            fi
        done < <(xml_elements "$file" component)
    done
    printf '%s\n' "$1"
}

# The component.xml name of the component in directory $1, else the directory name.
component_name_of() {
    local name
    name=$(xml_attr "$(xml_elements "$1/component.xml" component | head -n 1)" name)
    printf '%s\n' "${name:-$(basename "$1")}"
}

# The directory in runtime/components/addon of the component a depends-on of $1 means: the directory of that name, else the
# one whose component.xml has that name.
component_dir_named() {
    local dir
    if [ -e "$RUNTIME_DIR/$ADDON_SET/$1" ]; then
        printf '%s\n' "$RUNTIME_DIR/$ADDON_SET/$1"
        return 0
    fi
    for dir in "$RUNTIME_DIR"/$ADDON_SET/*; do
        if [ -f "$dir/component.xml" ] && [ "$(component_name_of "$dir")" = "$1" ]; then
            printf '%s\n' "$dir"
            return 0
        fi
    done
    return 1
}

# $1 the component, $2 the location type, $3 the component directory it goes in (runtime/components/addon unless --dev; the
# components it depends on always go in runtime/components/addon).
component_get_one() {
    local name type=$2 element target base=${3:-$RUNTIME_DIR/$ADDON_SET}
    name=$(addons_name_for "$1")
    case $COMPONENTS_CHECKED in *" $name "*) return 0 ;; esac
    element=$(addons_find component "$name") || die "component $name is not in myaddons.xml or addons.xml"
    if [ "$(xml_attr "$element" skip-get)" = true ]; then
        info "Skipping $name (skip-get=true)"
        COMPONENTS_CHECKED="$COMPONENTS_CHECKED$name "
        return 0
    fi
    target=$base/$name
    if [ -e "$target" ]; then
        info "$name is already in $(rel "$target")"
    else
        download_component "$name" "$type" "$element" "$target"
    fi
    component_check_depends "$name" "$type" "$target"
}

# A depends-on names a component by its component.xml name, which for a fork is not always its directory (a depends-on
# shop finds the fork in my-shop).
component_check_depends() {
    local name=$1 type=$2 dir=${3:-$RUNTIME_DIR/$ADDON_SET/$1} dep dep_dir line
    case $COMPONENTS_CHECKED in *" $name "*) return 0 ;; esac
    COMPONENTS_CHECKED="$COMPONENTS_CHECKED$name "
    [ -f "$dir/component.xml" ] || return 0
    local -a deps
    deps=()
    while IFS= read -r line; do
        dep=$(xml_attr "$line" name)
        [ -z "$dep" ] || deps+=("$dep")
    done < <(xml_elements "$dir/component.xml" depends-on)
    for dep in "${deps[@]}"; do
        if dep_dir=$(component_dir_named "$dep"); then
            component_check_depends "$(basename "$dep_dir")" "$type"
        else
            info "$name depends on $dep"
            component_get_one "$dep" "$type"
        fi
    done
}

components_checked_summary() {
    local list
    list=$(printf '%s' "$COMPONENTS_CHECKED" | tr -s ' ' | sed 's/^ //;s/ $//;s/ /, /g')
    [ -z "$list" ] || info "Checked: $list"
}

# Sets REPOSITORY and LOCATION_TYPE from -r and -t, COMPONENT_BASE from --dev (runtime/components/dev, else
# runtime/components/addon), and COMPONENT_ARGS from the other words.
parse_component_args() {
    LOCATION_TYPE=$(default_location_type)
    REPOSITORY=
    COMPONENT_BASE=$RUNTIME_DIR/$ADDON_SET
    COMPONENT_ARGS=()
    while [ $# -gt 0 ]; do
        case $1 in
            --dev) COMPONENT_BASE=$RUNTIME_DIR/$DEV_SET ;;
            --test) COMPONENT_BASE=$RUNTIME_DIR/$TEST_SET ;;
            -t | --type)
                [ $# -ge 2 ] || die "$1 needs a type"
                LOCATION_TYPE=$2
                shift
                ;;
            --type=*) LOCATION_TYPE=${1#--type=} ;;
            -r | --repo)
                [ $# -ge 2 ] || die "$1 needs a repository"
                REPOSITORY=$2
                shift
                ;;
            --repo=*) REPOSITORY=${1#--repo=} ;;
            -*) die "unknown option $1" ;;
            *) COMPONENT_ARGS+=("$1") ;;
        esac
        shift
    done
    check_location_type "$LOCATION_TYPE"
}

require_addons() {
    [ -f "$ADDONS" ] || [ -f "$MYADDONS" ] || die "no addons.xml or myaddons.xml in $(rel "$ROOT")"
    served_addons_load
}

# A server's addons.xml (a group of a hosting server serves one, of its public components with their clone addresses):
# a myaddons.xml repository's <location type="addons" url="..."/>. Each is fetched once per run, over https (http only
# to a loopback address, or a file: URL), and searched after myaddons.xml and before addons.xml, so its components and
# its repositories (the server's git, current and release locations) are found as if they were in myaddons.xml. Only
# its components and repositories are read: never its root's default or default-repository. One that cannot be
# fetched is skipped with a warning.
SERVED_ADDONS_DIR=
served_addons_load() {
    local url file n=0
    local -a urls served
    urls=()
    served=()
    while IFS= read -r url; do
        [ -z "$url" ] || urls+=("$url")
    done < <(xml_strip_comments "$MYADDONS" | awk '
        /<repository[[:space:]]/ { inrepo = 1 }
        inrepo && /<location[[:space:]]/ && /[[:space:]]type="addons"/ {
            if (match($0, /url="[^"]*"/)) print substr($0, RSTART + 5, RLENGTH - 6)
        }
        /<\/repository>/ { inrepo = 0 }')
    [ ${#urls[@]} -gt 0 ] || return 0
    need_tool curl
    SERVED_ADDONS_DIR=$(mktemp -d "${TMPDIR:-/tmp}/moq-addons.XXXXXX") || die "could not make a temp directory"
    trap 'rm -rf "$SERVED_ADDONS_DIR"' EXIT
    for url in "${urls[@]}"; do
        case $url in
            https://* | file://* | http://127.0.0.1[:/]* | http://localhost[:/]* | "http://[::1]"[:/]*) ;;
            *)
                warn "myaddons.xml: the addons.xml at $url is not https (http only to a loopback address); skipped"
                continue
                ;;
        esac
        n=$((n + 1))
        file=$SERVED_ADDONS_DIR/served-$n.xml
        if curl -fsSL --proto '=https,http,file' --proto-redir '=https' --max-time 30 --max-filesize 4194304 \
            -o "$file" --url "$url" && xml_elements "$file" addons >/dev/null; then
            served+=("$file")
        else
            warn "myaddons.xml: could not read an addons.xml from $url; skipped"
        fi
    done
    ADDONS_FILES=("$MYADDONS" ${served[@]+"${served[@]}"} "$ADDONS")
}

task_component_get() {
    local name defaults
    parse_component_args "$@"
    require_addons
    if [ ${#COMPONENT_ARGS[@]} -eq 0 ]; then
        defaults=$(addons_root_attr "$MYADDONS" default)
        [ -n "$defaults" ] || die "give component names, or set default=\"a,b\" on the myaddons.xml root element"
        COMPONENT_ARGS=()
        for name in $(printf '%s\n' "$defaults" | tr ',' ' '); do
            COMPONENT_ARGS+=("$name")
        done
    fi
    run_cmd mkdir -p "$COMPONENT_BASE"
    for name in "${COMPONENT_ARGS[@]}"; do
        if addons_find component "$(addons_name_for "$name")" >/dev/null; then
            component_get_one "$name" "$LOCATION_TYPE" "$COMPONENT_BASE"
        elif addons_find component-set "$name" >/dev/null; then
            component_set_get "$name" "$LOCATION_TYPE" "$COMPONENT_BASE"
        else
            die "$name is not a component or component-set in myaddons.xml or addons.xml"
        fi
    done
    components_checked_summary
}

# $1 the component-set, $2 the location type, $3 the component directory its components go in.
component_set_get() {
    local set=$1 type=$2 base=$3 element name
    element=$(addons_find component-set "$set") || die "component-set $set is not in myaddons.xml or addons.xml"
    for name in $(xml_attr "$element" components | tr ',' ' '); do
        component_get_one "$name" "$type" "$base"
    done
    for name in $(xml_attr "$element" sets | tr ',' ' '); do
        component_set_get "$name" "$type" "$base"
    done
}

component_dirs() {
    local dir
    for dir in "$RUNTIME_DIR"/$ADDON_SET/*; do
        if [ -f "$dir/component.xml" ]; then
            printf '%s\n' "$dir"
        fi
    done
}

task_component_depends() {
    local dir
    local -a dirs
    parse_component_args "$@"
    [ ${#COMPONENT_ARGS[@]} -eq 0 ] || die "component-depends takes no component names"
    require_addons
    dirs=()
    while IFS= read -r dir; do
        dirs+=("$dir")
    done < <(component_dirs)
    for dir in "${dirs[@]}"; do
        component_check_depends "$(basename "$dir")" "$LOCATION_TYPE"
    done
    components_checked_summary
}

# The forms of the component name $1 that the template placeholders stand for (runtime/components/dev/README.md):
# @component@ my-shop, @Title@ My Shop, @Class@ MyShop, @package@ myshop, @ID@ MY_SHOP. The Dev AIDE makes the same
# (dev-aide src/template.rs).
name_title() {
    printf '%s\n' "$1" | awk -F'[-_]' -v sep="${2- }" '{
        out = ""
        for (i = 1; i <= NF; i++) if ($i != "") out = out (out == "" ? "" : sep) toupper(substr($i, 1, 1)) substr($i, 2)
        print out
    }'
}
name_class() { name_title "$1" ''; }
name_package() { printf '%s\n' "$1" | tr -d '_-' | tr 'A-Z' 'a-z'; }
name_id() { printf '%s\n' "$1" | tr 'a-z-' 'A-Z_'; }

# Whether $1 can be a component name a template fills in: a letter, then letters, digits, '-' and '_', so each form is
# a name in XML, a package and a URL.
template_name() {
    case $1 in
        [A-Za-z]*) ;;
        *) return 1 ;;
    esac
    case $1 in
        *[!A-Za-z0-9_-]*) return 1 ;;
    esac
    return 0
}

# The directory of the template $1: one in runtime/components/dev or runtime/components/addon whose component.xml is named
# @component@.
component_template() {
    local dir
    for dir in "$RUNTIME_DIR/$DEV_SET/$1" "$RUNTIME_DIR/$ADDON_SET/$1"; do
        if [ -f "$dir/component.xml" ] && grep -q 'name="@component@"' "$dir/component.xml"; then
            printf '%s\n' "$dir"
            return 0
        fi
    done
    return 1
}

# The templates for --from completion.
component_template_names() {
    local dir
    for dir in "$RUNTIME_DIR"/$DEV_SET/*/ "$RUNTIME_DIR"/$ADDON_SET/*/; do
        grep -qs 'name="@component@"' "${dir}component.xml" && basename "$dir"
    done
}

# The files a part leaves out of a new component, by their template paths: --rest has no screens, --screens no REST API.
template_part_files() {
    case $1 in
        rest) printf '%s\n' screen MoquiConf.xml data/@Class@SecurityData.xml test/@Class@Screens.spec.js \
            test/acceptance ;;
        screens) printf '%s\n' service/@component@.rest.xml data/@Class@ApiSecurityData.xml test/@Class@Api.spec.js ;;
    esac
}

task_component_create() {
    local name='' from=template part='' remote='' no_git='' group='' template target file dir base new branch
    local component_base=$RUNTIME_DIR/$ADDON_SET
    while [ $# -gt 0 ]; do
        case $1 in
            --dev) component_base=$RUNTIME_DIR/$DEV_SET ;;
            --test) component_base=$RUNTIME_DIR/$TEST_SET ;;
            --from)
                [ $# -ge 2 ] || die "--from needs a template name"
                from=$2
                shift
                ;;
            --rest) part=rest ;;
            --screens) part=screens ;;
            --git-remote)
                [ $# -ge 2 ] || die "--git-remote needs a URL"
                remote=$2
                shift
                ;;
            --no-git) no_git=1 ;;
            --myaddons)
                [ $# -ge 2 ] || die "--myaddons needs a git group"
                group=$2
                shift
                ;;
            -*) die "unknown option $1" ;;
            *)
                [ -z "$name" ] || die "component-create takes one component name"
                name=$1
                ;;
        esac
        shift
    done
    [ -n "$name" ] || die "component-create needs a component name"
    template_name "$name" || die "component name '$name' starts with a letter and has letters, digits, '-' and '_'"
    [ -z "$remote" ] || [ -z "$no_git" ] || die "--git-remote and --no-git do not go together"
    template=$(component_template "$from") \
        || die "no template $from in runtime/components/dev or runtime/components/addon (a component.xml named @component@)"
    target=$component_base/$name
    [ ! -e "$target" ] || die "$(rel "$target") already exists"
    # The template's contents, so a template reached through a link is copied and not the link.
    run_cmd mkdir -p "$target"
    run_cmd cp -R "$template/." "$target" || die "could not copy $(rel "$template") to $(rel "$target")"
    if dry_run; then
        info "(dry run: filling in the placeholders in $(rel "$target") is skipped)"
        return 0
    fi
    rm -rf "$target/.git"
    while IFS= read -r file; do
        rm -rf "${target:?}/$file"
    done < <(template_part_files "$part")

    local title class package id
    title=$(name_title "$name")
    class=$(name_class "$name")
    package=$(name_package "$name")
    id=$(name_id "$name")
    # Contents first, then the names, deepest first so a renamed directory does not move a file still to be renamed.
    while IFS= read -r file; do
        sed -e "s/@component@/$name/g" -e "s/@Title@/$title/g" -e "s/@Class@/$class/g" -e "s/@package@/$package/g" \
            -e "s/@ID@/$id/g" "$file" >"$file.moq-tmp" && cat "$file.moq-tmp" >"$file"
        rm -f "$file.moq-tmp"
    done < <(grep -rlI -e '@component@' -e '@Title@' -e '@Class@' -e '@package@' -e '@ID@' "$target")
    while IFS= read -r file; do
        dir=$(dirname "$file")
        base=$(basename "$file")
        new=$(printf '%s\n' "$base" | sed -e "s/@component@/$name/g" -e "s/@Class@/$class/g" \
            -e "s/@package@/$package/g" -e "s/@ID@/$id/g")
        mv "$file" "$dir/$new"
    done < <(find "$target" -depth -name '*@*')
    # The template's own disabled="true", which keeps it from loading as a component.
    sed 's/ disabled="true"//' "$target/component.xml" >"$target/component.xml.moq-tmp" \
        && cat "$target/component.xml.moq-tmp" >"$target/component.xml"
    rm -f "$target/component.xml.moq-tmp"
    case $part in
        rest) info "REST API only: no screens" ;;
        screens) info "Screens only: no REST API" ;;
    esac

    branch=master
    if [ -z "$no_git" ]; then
        need_tool git
        if ! git -C "$target" init -q || ! git -C "$target" add .; then
            die "git init failed in $(rel "$target")"
        fi
        branch=$(git -C "$target" symbolic-ref --short HEAD 2>/dev/null || echo master)
        if [ -n "$remote" ]; then
            git -C "$target" remote add origin "$remote"
        fi
    fi
    if [ -n "$group" ]; then
        add_to_myaddons "$name" "$group" "$branch"
    fi
    info "Created $(rel "$target") from $from"
    if [ -z "$no_git" ]; then
        info "Next: cd $(rel "$target") && git commit -m 'Initial commit'${remote:+ && git push -u origin $branch}"
    fi
}

add_to_myaddons() {
    local name=$1 group=$2 branch=$3 line
    line="    <component name=\"$name\" group=\"$group\" branch=\"$branch\"/>"
    if [ ! -f "$MYADDONS" ]; then
        printf '<addons>\n%s\n</addons>\n' "$line" >"$MYADDONS"
        info "Created myaddons.xml with $name"
        return 0
    fi
    grep -q '</addons>' "$MYADDONS" || die "myaddons.xml has no </addons> to add $name before"
    awk -v line="$line" '
        { lines[NR] = $0; if (index($0, "</addons>")) last = NR }
        END { for (i = 1; i <= NR; i++) { if (i == last) print line; print lines[i] } }
    ' "$MYADDONS" >"$MYADDONS.moq-tmp" && cat "$MYADDONS.moq-tmp" >"$MYADDONS"
    rm -f "$MYADDONS.moq-tmp"
    info "Added $name to myaddons.xml"
}

task_component_zip() {
    local dir name element cname version zip with_tests=''
    local -a dirs names
    dirs=()
    names=()
    while [ $# -gt 0 ]; do
        case $1 in
            --with-tests) with_tests=1 ;;
            -*) die "unknown option $1" ;;
            *) names+=("$1") ;;
        esac
        shift
    done
    set -- ${names[@]+"${names[@]}"}
    need_tool zip
    if [ $# -eq 0 ]; then
        while IFS= read -r dir; do
            dirs+=("$dir")
        done < <(component_dirs)
        [ ${#dirs[@]} -gt 0 ] || die "no components with a component.xml in $(rel "$RUNTIME_DIR/$ADDON_SET")"
    else
        for name in "$@"; do
            dir=$RUNTIME_DIR/$ADDON_SET/$name
            [ -f "$dir/component.xml" ] || die "no component.xml in $(rel "$dir")"
            dirs+=("$dir")
        done
    fi
    for dir in "${dirs[@]}"; do
        name=$(basename "$dir")
        element=$(xml_elements "$dir/component.xml" component | head -n 1)
        cname=$(xml_attr "$element" name)
        version=$(xml_attr "$element" version)
        zip=$RUNTIME_DIR/$ADDON_SET/${cname:-$name}${version:+-$version}.zip
        if [ -e "$zip" ]; then
            rm_path "$zip"
        fi
        if dry_run; then
            show_cmd cd "$RUNTIME_DIR/$ADDON_SET"
        fi
        # A Rust component (a Cargo.toml) ships its source, which the binary is built from; a Java one leaves it out.
        local -a leave_out
        if [ -f "$dir/Cargo.toml" ]; then
            leave_out=("$name/target/*" "$name/Cargo.lock")
        else
            leave_out=("$name/src/*")
        fi
        [ -n "$with_tests" ] || leave_out+=("$name/test" "$name/test/*")
        (cd "$RUNTIME_DIR/$ADDON_SET" && run_cmd zip -qr "$zip" "$name" \
            -x "$name/.git" "$name/.git/*" "$name/build/*" "${leave_out[@]}" "$name/librepo/*" "$name/build.gradle" \
            "*/.DS_Store" "*~") || die "zip failed for $name"
        info "Created $(rel "$zip")"
    done
}


# ========== help ==========

help_component_get() {
    cat <<EOF
Gets each named component into runtime/components/addon from the location in myaddons.xml or
addons.xml, then gets any component it depends on (component.xml depends-on) that is not
there yet. A component already present is not changed, but its dependencies are checked.
A name that is a component-set, and not a component, gets the components of the set,
including nested sets. With no names it gets the default="a,b" list on the myaddons.xml
root element.
  ./moq component-get mr-mantle
  ./moq component-get -t current mr-boss
  ./moq component-get apps

Options:
      --dev            Get the named components into runtime/components/dev, the components that load
                       only when instance@purpose is dev; what they depend on still goes in
                       runtime/components/addon
      --test           Get them into runtime/components/test, the test-only components that load on
                       a dev or test instance, never on production and not in ./moq archive unless
                       --with-tests; what they depend on still goes in runtime/components/addon
$COMPONENT_TYPE_HELP
$COMPONENT_REPO_HELP
EOF
}

help_component_depends() {
    cat <<EOF
Checks every component in runtime/components/addon and gets the depends-on components that are missing.

Options:
$COMPONENT_TYPE_HELP
$COMPONENT_REPO_HELP
EOF
}

help_component_create() {
    cat <<EOF
Creates runtime/components/addon/<name> from the component template, runtime/components/dev/template:
copies it, fills in the placeholders in its file names and contents, and takes out the
template's disabled="true". For the name my-shop the placeholders are @component@ my-shop,
@Title@ My Shop, @Class@ MyShop, @package@ myshop and @ID@ MY_SHOP. A name starts with a letter
and has letters, digits, '-' and '_'. The new component loads at the next start.
  ./moq component-create my-shop
  ./moq component-create my-api --rest --git-remote git@github.com:me/my-api.git --myaddons me

Options:
      --dev            Create it in runtime/components/dev, loaded only when instance@purpose is dev
      --test           Create it in runtime/components/test, loaded on a dev or test instance only
      --from NAME      Another template, in runtime/components/dev or runtime/components/addon (a component.xml
                       named @component@)
      --rest           REST API only: no screen/, MoquiConf.xml, or screen security data and spec
      --screens        Screens only: no REST API (service/<name>.rest.xml) or its security data and spec
      --git-remote URL git init, git add ., and add this remote as origin
      --no-git         Do not git init
      --myaddons GROUP Add the component to myaddons.xml with this git group (creating the file if needed)
EOF
}

help_component_zip() {
    cat <<'EOF'
Writes runtime/components/addon/<name>-<version>.zip for each named component, or for every directory
in runtime/components/addon with a component.xml. It leaves out .git, build/, src/, librepo/,
build.gradle and, unless --with-tests, test/ (the regression and acceptance suites, agent replays and
load scripts), so a zip for production carries no tests. Needs zip.

Options:
      --with-tests     Keep the component's test/
EOF
}
