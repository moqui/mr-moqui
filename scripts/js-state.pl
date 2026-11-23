# Check an installed webroot without starting Node. Perl and its core modules ship with Git Bash too.
use strict;
use warnings;
use Digest::SHA qw(sha256_hex);
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

my $manifest = decode_json(bytes("$libs/MANIFEST.json"));
my %want = ('MANIFEST.json' => undef);
for my $f ($manifest->{notices}, map { @{$_->{files}} } @{$manifest->{libraries}}) {
    $want{$f->{path}} = $f->{sha256};
}
my $stamp = "$cache/stamp.json";
my $inputs = Digest::SHA->new(256);
for my $path (files($source)) {
    $inputs->add($path, "\0", sha256_hex(bytes("$source/$path")), "\n");
}
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
        exit 0;
    }
    # A packaged or read-only tree can be verified directly against its manifest.
    exit 0 unless -w $cache || !-e $cache && -w "$cache/..";
} elsif ($action ne 'stamp') {
    die "Unknown state operation $action\n";
}
make_path($cache);
open my $out, '>:raw', "$stamp.$$" or die "$stamp: $!";
print $out encode_json({ inputs => $signature, node => $ENV{JS_NODE_VERSION}, npm => $ENV{JS_NPM_VERSION} });
close $out or die "$stamp: $!";
rename "$stamp.$$", $stamp or die "$stamp: $!";
