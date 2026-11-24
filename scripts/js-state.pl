# Check an installed webroot without starting Node. Perl and its core modules ship with Git Bash too.
use strict;
use warnings;
use Digest::SHA qw(sha256_hex);
use File::Basename qw(dirname);
use File::Find;
use File::Path qw(make_path);
use JSON::PP;

my ($action, $source, $libs, $cache) = @ARGV;
sub bytes {
    open my $fh, '<:raw', $_[0] or die "$_[0]: $!\n";
    local $/;
    return <$fh>;
}

# The components with browser libraries to build: each <set>/<component>/build/js-build.json under the components
# directory, the sets in the order components.xml lists them, then any others (base first). One line each: name, root, output,
# script and the projects, tab separated.
if ($action eq 'components') {
    my $dir = $source;
    my @sets;
    if (-f "$dir/components.xml") { @sets = bytes("$dir/components.xml") =~ /<component-dir\s+location="([^"\/]+)"/g }
    my %seen = map { $_ => 1 } @sets;
    opendir my $dh, $dir or die "$dir: $!\n";
    # sets components.xml does not list follow, the framework's base set first
    push @sets, sort { ($b eq 'base') <=> ($a eq 'base') || $a cmp $b } grep { !$seen{$_} && !/^\./ && -d "$dir/$_" } readdir $dh;
    closedir $dh;
    my %names;
    for my $set (@sets) {
        next unless opendir my $sh, "$dir/$set";
        for my $name (sort grep { !/^\./ } readdir $sh) {
            my $root = "$dir/$set/$name";
            my $config = "$root/build/js-build.json";
            next unless -f $config;
            my $c = eval { decode_json(bytes($config)) } or die "$config: not JSON\n";
            my $relative = qr{^(?!/)(?!.*(?:^|/)\.\.(?:/|$))[\w.\/-]+$};
            die "$config: output must be a path under screen/\n" unless ($c->{output} // '') =~ $relative && $c->{output} =~ m{^screen/};
            $c->{script} = ':standard' if !defined $c->{script} && ref($c->{libraries}) eq 'ARRAY';
            die "$config: script must be a path under build/\n" unless ($c->{script} // '') eq ':standard' || ($c->{script} // '') =~ $relative;
            my @projects = @{ $c->{projects} // [] };
            for (@projects) { die "$config: project $_ must be a directory name\n" unless /^[\w.-]+$/ && !/^\.\.?$/ }
            die "$config: component $name is also in $names{$name}\n" if $names{$name};
            $names{$name} = $set;
            print join("\t", $name, $root, $c->{output}, $c->{script}, join(',', @projects)), "\n";
        }
        closedir $sh;
    }
    exit 0;
}
sub files {
    my ($dir) = @_;
    my @paths;
    find({ no_chdir => 1, wanted => sub {
        die "Unexpected asset symlink $_\n" if -l;
        push @paths, substr($_, length($dir) + 1) if -f;
    } }, $dir);
    return sort @paths;
}

my $font_config = "$source/fonts/fonts.json";
my $packs = -f $font_config ? decode_json(bytes($font_config)) : {};
my $selection = "$source/fonts/selected.txt";
# Font packs belong to the component with a fonts.json (the webroot); another component has none to select.
my %selected = !-f $font_config ? () : map { $_ => 1 } grep { length } split /[\s,]+/,
    $ENV{MOQUI_FONT_PACKS} // (-f $selection ? bytes($selection) : '');
for my $name (keys %selected) { die "Unknown font pack $name\n" unless exists $packs->{$name}; }
if ($action eq 'selected') { print join(' ', sort keys %selected), "\n"; exit 0; }
if ($action eq 'packs') { print join("\n", sort keys %$packs), "\n"; exit 0; }
my $manifest = decode_json(bytes("$libs/MANIFEST.json"));
my %want = ('MANIFEST.json' => undef);
my $server = dirname($source) . '/script/libs';
my %server_want;
for my $f (grep { defined } $manifest->{notices}, map { @{$_->{files}} } @{$manifest->{libraries}}) {
    next if $f->{pack} && !$selected{$f->{pack}};
    if (($f->{target} // '') eq 'server') {
        die "Invalid server module path" unless $f->{path} =~ /^[\w-]+\.js$/;
        $server_want{$f->{path}} = $f->{sha256};
        next;
    }
    $want{$f->{path}} = $f->{sha256};
}
if (-f $font_config) {
    $want{'fonts-extra/installed.json'} = sha256_hex(encode_json([sort keys %selected]) . "\n");
}
if ($action eq 'server-files') { print "$server/$_\n" for sort keys %server_want; exit 0; }
my $stamp = "$cache/stamp.json";
my $inputs = Digest::SHA->new(256);
for my $path (files($source)) {
    next if $path eq "fonts/selected.txt";
    $inputs->add($path, "\0", sha256_hex(bytes("$source/$path")), "\n");
}
if ($ENV{JS_SHARED_BUILD} && decode_json(bytes("$source/js-build.json"))->{libraries}) {
    for my $name (qw(component-libs.mjs js-kit.mjs tools.mjs)) { $inputs->add(bytes("$ENV{JS_SHARED_BUILD}/$name")); }
}
$inputs->add(join(" ", sort keys %selected));
$inputs->add(sha256_hex(bytes("$libs/MANIFEST.json")));
my $signature = $inputs->hexdigest;

if ($action eq 'check') {
    for my $path (keys %server_want) { exit 1 unless -f "$server/$path" && sha256_hex(bytes("$server/$path")) eq $server_want{$path}; }
    my @actual = files($libs);
    for my $path (@actual) {
        die "Unexpected JS asset $path\n" unless exists $want{$path};
        next unless defined $want{$path};
        exit 1 unless sha256_hex(bytes("$libs/$path")) eq $want{$path};
    }
    exit 1 unless @actual == keys %want;
    if (-f $stamp) {
        my $saved = eval { decode_json(bytes($stamp)) };
        exit 1 unless $saved && $saved->{inputs} eq $signature;
        exit 0 if exists $saved->{outputs};
        $ENV{JS_NODE_VERSION} = $saved->{node} if defined $saved->{node};
        $ENV{JS_NPM_VERSION} = $saved->{npm} if defined $saved->{npm};
    }
    # A packaged or read-only tree can be verified directly against its manifest.
    exit 0 unless -w $cache || !-e $cache && -w dirname($cache);
} elsif ($action ne 'stamp') {
    die "Unknown state operation $action\n";
}
make_path($cache);
my %esbuild;
opendir my $pd, $source or die "$source: $!";
for my $project (sort grep { -f "$source/$_/package-lock.json" } readdir $pd) {
    my $lock = "$source/$project/package-lock.json";
    $esbuild{$project} = decode_json(bytes($lock))->{packages}{'node_modules/esbuild'}{version} if -f $lock;
}
$want{'MANIFEST.json'} = sha256_hex(bytes("$libs/MANIFEST.json"));
open my $out, '>:raw', "$stamp.$$" or die "$stamp: $!";
print $out JSON::PP->new->canonical->encode({
    inputs => $signature, node => $ENV{JS_NODE_VERSION}, npm => $ENV{JS_NPM_VERSION},
    esbuild => \%esbuild, packs => [sort keys %selected], outputs => \%want,
    (%server_want ? (serverOutputs => \%server_want) : ()),
});
close $out or die "$stamp: $!";
rename "$stamp.$$", $stamp or die "$stamp: $!";
