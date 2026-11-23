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
my %selected = map { $_ => 1 } grep { length } split /[\s,]+/,
    $ENV{MOQUI_FONT_PACKS} // (-f $selection ? bytes($selection) : '');
for my $name (keys %selected) { die "Unknown font pack $name\n" unless exists $packs->{$name}; }
if ($action eq 'selected') { print join(' ', sort keys %selected), "\n"; exit 0; }
if ($action eq 'packs') { print join("\n", sort keys %$packs), "\n"; exit 0; }
my $manifest = decode_json(bytes("$libs/MANIFEST.json"));
my %want = ('MANIFEST.json' => undef);
for my $f ($manifest->{notices}, map { @{$_->{files}} } @{$manifest->{libraries}}) {
    next if $f->{pack} && !$selected{$f->{pack}};
    $want{$f->{path}} = $f->{sha256};
}
if (-f $font_config) {
    $want{'fonts-extra/installed.json'} = sha256_hex(encode_json([sort keys %selected]) . "\n");
}
my $stamp = "$cache/stamp.json";
my $inputs = Digest::SHA->new(256);
for my $path (files($source)) {
    next if $path eq "fonts/selected.txt";
    $inputs->add($path, "\0", sha256_hex(bytes("$source/$path")), "\n");
}
$inputs->add(join(" ", sort keys %selected));
$inputs->add(sha256_hex(bytes("$libs/MANIFEST.json")));
my $signature = $inputs->hexdigest;

if ($action eq 'check') {
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
for my $project (qw(libs whiteboard)) {
    my $lock = "$source/$project/package-lock.json";
    $esbuild{$project} = decode_json(bytes($lock))->{packages}{'node_modules/esbuild'}{version} if -f $lock;
}
$want{'MANIFEST.json'} = sha256_hex(bytes("$libs/MANIFEST.json"));
open my $out, '>:raw', "$stamp.$$" or die "$stamp: $!";
print $out JSON::PP->new->canonical->encode({
    inputs => $signature, node => $ENV{JS_NODE_VERSION}, npm => $ENV{JS_NPM_VERSION},
    esbuild => \%esbuild, packs => [sort keys %selected], outputs => \%want,
});
close $out or die "$stamp: $!";
rename "$stamp.$$", $stamp or die "$stamp: $!";
