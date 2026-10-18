#!/bin/bash
# macOS profile handoff. Copies only; never removes the original profile.
set -eo pipefail
umask 077
usage() {
  cat <<'USAGE'
Usage: bash migrate-monocode-to-vorkspace.sh [--apply] [--backup-dir PATH]
       [--source PROFILE --destination PROFILE]

Dry-run is the default. Quit MonoCode, vorkspace and any local host first.
vorkspace already uses MonoCode's com.monocode.desktop profile on this Mac.
--apply makes a private backup; the default handoff needs no profile move.
An optional recovery copy requires both paths and an absent/empty destination.
The normal app consumes ~/Library/Application Support/com.monocode.desktop.
Keychain stays on this Mac; external provider homes and workspaces are not copied.
Symlinks are preserved, not followed. Originals are never deleted.
USAGE
}
fail() { printf '%s\n' "$*" >&2; exit 1; }
apply=0 source_profile='' destination='' backup=''
while [ "$#" -gt 0 ]; do
  case "$1" in
    --apply) apply=1; shift ;;
    --dry-run) apply=0; shift ;;
    --backup-dir|--source|--destination)
      [ "$#" -ge 2 ] && [ -n "$2" ] || fail "Missing path for $1"
      case "$1" in
        --backup-dir) backup=$2 ;;
        --source) source_profile=$2 ;;
        --destination) destination=$2 ;;
      esac
      shift 2 ;;
    --help|-h) usage; exit 0 ;;
    *) fail "Unknown argument: $1" ;;
  esac
done
[ "$(uname -s)" = Darwin ] || fail 'This helper supports macOS only.'
[ -n "${HOME:-}" ] && [ -d "$HOME" ] || fail 'HOME must be an existing directory.'
home=$(cd "$HOME" && pwd -P)
# Normalize paths using the nearest existing parent, without creating anything.
absolute() {
  local path=$1 suffix='' leaf
  case "$path" in /*) ;; *) fail 'Use absolute paths.' ;; esac
  while [ ! -e "$path" ] && [ ! -L "$path" ]; do
    leaf=$(basename "$path")
    case "$leaf" in .|..) fail 'Use normalized paths without dot components.' ;; esac
    suffix="/$leaf$suffix"; path=$(dirname "$path")
  done
  [ -d "$path" ] || fail 'A path parent is not a directory.'
  printf '%s%s\n' "$(cd "$path" && pwd -P)" "$suffix"
}
if [ -n "$source_profile" ] || [ -n "$destination" ]; then
  [ -n "$source_profile" ] && [ -n "$destination" ] || fail 'Recovery requires --source and --destination together.'
  [ -d "$source_profile" ] && [ ! -L "$source_profile" ] || fail 'Recovery source must be a real directory.'
  source_profile=$(absolute "$source_profile")
  [ ! -L "$destination" ] || fail 'Recovery destination cannot be a symlink.'
  destination=$(absolute "$destination")
  case "$destination/" in "$source_profile/"*) fail 'Recovery destination overlaps its source.' ;; esac
  case "$source_profile/" in "$destination/"*) fail 'Recovery destination overlaps its source.' ;; esac
  if [ -e "$destination" ]; then
    [ -d "$destination" ] || fail 'Recovery destination must be a directory.'
    [ -z "$(find "$destination" -mindepth 1 -maxdepth 1 -print -quit)" ] || fail 'Recovery destination is populated; refusing to overwrite it.'
  fi
fi
backup=${backup:-"$home/vorkspace-backups/$(date -u +%Y%m%dT%H%M%SZ)-$$"}
[ ! -e "$backup" ] && [ ! -L "$backup" ] || fail 'Backup destination already exists.'
backup=$(absolute "$backup")
paths=()
add() { [ ! -e "$home/$1" ] && [ ! -L "$home/$1" ] || paths+=("$1"); }
for relative in \
  'Library/Application Support/com.monocode.desktop' \
  'Library/Preferences/com.monocode.desktop.plist' \
  'Library/WebKit/com.monocode.desktop' \
  'Library/Caches/com.monocode.desktop' \
  'Library/HTTPStorages/com.monocode.desktop' \
  'Library/HTTPStorages/com.monocode.desktop.binarycookies' \
  'Library/Cookies/com.monocode.desktop.binarycookies' \
  'Library/Saved Application State/com.monocode.desktop.savedState' \
  'Library/LaunchAgents/com.monocode.host.plist'; do add "$relative"; done
shopt -s nullglob
for path in "$home"/.monocode* "$home"/Library/Preferences/ByHost/com.monocode.desktop.*.plist; do
  add "${path#"$home/"}"
done
for relative in "${paths[@]}"; do
  case "$backup/" in "$home/$relative/"*) fail 'Backup must be outside all source paths.' ;; esac
done
if [ -n "$source_profile" ]; then
  case "$backup/" in "$source_profile/"*) fail 'Backup must be outside the recovery source.' ;; esac
  case "$backup/" in "$destination/"*) fail 'Backup must be outside the recovery destination.' ;; esac
  case "$destination/" in "$backup/"*) fail 'Recovery destination must be outside the backup.' ;; esac
fi
run_migration_perl() {
  /usr/bin/perl - "$@" <<'PERL'
use strict;
use warnings;
use File::Find;
use Encode qw(encode decode);
use JSON::PP;

my %EXACT_KEYS = (
    'monocode:mono-roster'          => 'monocode:bot-roster',
    'monocode:mono-handed'          => 'monocode:bot-handed',
    'monocode:monos'                => 'monocode:bots',
    'monocode:mono-profiles'        => 'monocode:bot-profiles',
    'monocode:open-mono'            => 'monocode:open-bot',
    'monocode:mono-declined-habits' => 'monocode:bot-declined-habits',
    'monocode:mono-channels'        => 'monocode:bot-channels',
    'monocode:open-mono-channel'    => 'monocode:open-bot-channel',
    'monocode:mono-rotations'       => 'monocode:bot-rotations',
    'monocode:mono-intro-dismissed' => 'monocode:bot-intro-dismissed',
    'monocode.inboxMonos.v1'        => 'monocode.inboxBots.v1',
    'monocode.railMonosPinned'      => 'monocode.railBotsPinned',
    'monocode.monosEnabled'         => 'monocode.botsEnabled',
    'monocode.monoMenuBarIcon'      => 'monocode.botMenuBarIcon',
);

sub is_sqlite3 {
    my ($file) = @_;
    return 0 unless defined $file && -f $file && ! -l $file && -r $file;
    open my $fh, '<:raw', $file or return 0;
    my $header;
    my $bytes = read($fh, $header, 16);
    close $fh;
    return 0 unless defined $bytes && $bytes == 16;
    return ($header eq "SQLite format 3\0") ? 1 : 0;
}

sub table_exists {
    my ($db, $table) = @_;
    open my $fh, "-|", "/usr/bin/sqlite3", "-batch", $db, "SELECT 1 FROM sqlite_master WHERE type='table' AND name='$table';" or return 0;
    my $found = <$fh>;
    close $fh;
    return $found ? 1 : 0;
}

sub find_webkit_dbs {
    my ($dir) = @_;
    return () unless defined $dir && -d $dir;
    my @dbs;
    File::Find::find({
        wanted => sub {
            return unless -f $_ && ! -l $_;
            my ($leaf) = $_ =~ m{([^/]+)\z};
            return unless defined $leaf && ($leaf eq 'localstorage.sqlite3' || $leaf =~ /\.localstorage\z/);
            push @dbs, $_ if is_sqlite3($_);
        },
        no_chdir => 1,
    }, $dir);
    return @dbs;
}

sub localstorage_migrations {
    my ($file) = @_;
    return () unless table_exists($file, 'ItemTable');

    open my $kh, "-|", "/usr/bin/sqlite3", "-batch", "-tabs", $file, "SELECT key, hex(value) FROM ItemTable;" or die "Failed to query ItemTable: $!\n";
    my %rows;
    while (<$kh>) {
        chomp;
        my ($key, $hex) = split /\t/, $_, 2;
        $rows{$key} = $hex if defined $key && defined $hex;
    }
    close $kh or die "Failed to read ItemTable\n";

    # After the app has claimed the new namespace, an empty collection can be
    # an explicit deletion. A later backup must never resurrect the old roster.
    if (exists $rows{'vorkspace.botStorageMigration.v1'}) {
        return () if decode('UTF-16LE', pack('H*', $rows{'vorkspace.botStorageMigration.v1'})) eq '1';
    }

    # Only known collections can be launch-initialized empty destinations.
    # Scalars, malformed JSON and mismatched shapes may be deliberate new data.
    my %collections = (
        'monocode:mono-roster' => 'ARRAY',
        'monocode:mono-handed' => 'HASH',
        'monocode:monos' => 'HASH',
        'monocode:mono-profiles' => 'HASH',
        'monocode:mono-declined-habits' => 'ARRAY',
        'monocode:mono-channels' => 'ARRAY',
        'monocode:mono-rotations' => 'HASH',
        'monocode.inboxMonos.v1' => 'HASH',
    );
    my @copies;
    for my $key ('monocode:project-chat-backgrounds', 'monocode.keybindingOverrides') {
        next unless exists $rows{$key};
        my $raw = pack("H*", $rows{$key});
        my $converted = transform_localstorage_value($key, $raw);
        push @copies, [$key, $key, $rows{$key}, $rows{$key}] if $converted ne $raw;
    }
    for my $old (keys %rows) {
        my $new = $EXACT_KEYS{$old};
        my $kind = $collections{$old};
        if ($old =~ /^monocode:mono-channel-messages:(.+)$/) {
            $new = "monocode:bot-channel-messages:$1";
            $kind = 'ARRAY';
        }
        next unless defined $new;
        if (exists $rows{$new}) {
            next unless defined $kind;
            my $source = eval { JSON::PP->new->decode(decode("UTF-16LE", pack("H*", $rows{$old}))) };
            my $dest = eval { JSON::PP->new->decode(decode("UTF-16LE", pack("H*", $rows{$new}))) };
            next unless ref($source) eq $kind && ref($dest) eq $kind;
            my $source_count = $kind eq 'ARRAY' ? scalar(@$source) : scalar(keys %$source);
            my $dest_count = $kind eq 'ARRAY' ? scalar(@$dest) : scalar(keys %$dest);
            next unless $source_count && !$dest_count;
        }
        push @copies, [$old, $new, $rows{$old}, $rows{$new}];
    }
    return @copies;
}

sub webkit_db_has_unmigrated {
    my @copies = localstorage_migrations(@_);
    return @copies ? 1 : 0;
}

sub transform_localstorage_value {
    my ($key, $raw_bytes) = @_;
    my $json_text = eval { decode("UTF-16LE", $raw_bytes) };
    return $raw_bytes unless defined $json_text;

    my $data = eval { JSON::PP->new->utf8(0)->decode($json_text) };
    return $raw_bytes unless defined $data;

    if ($key eq 'monocode:mono-channels') {
        if (ref($data) eq 'ARRAY') {
            for my $chan (@$data) {
                if (ref($chan) eq 'HASH') {
                    if (exists $chan->{monoIds} && !exists $chan->{botIds}) {
                        $chan->{botIds} = delete $chan->{monoIds};
                    }
                }
            }
        }
    } elsif ($key =~ /^monocode:mono-channel-messages:/) {
        if (ref($data) eq 'ARRAY') {
            for my $msg (@$data) {
                next unless ref($msg) eq 'HASH';
                for my $field ('from', 'author') {
                    if (ref($msg->{$field}) eq 'HASH') {
                        my $target = $msg->{$field};
                        if (defined $target->{kind} && $target->{kind} eq 'mono') {
                            $target->{kind} = 'bot';
                        }
                        if (exists $target->{monoId} && !exists $target->{botId}) {
                            $target->{botId} = delete $target->{monoId};
                        }
                    }
                }
                if (ref($msg->{reactions}) eq 'ARRAY') {
                    for my $rx (@{$msg->{reactions}}) {
                        if (ref($rx) eq 'HASH') {
                            if (exists $rx->{monoId} && !exists $rx->{botId}) {
                                $rx->{botId} = delete $rx->{monoId};
                            }
                        }
                    }
                }
            }
        }
    } elsif ($key eq 'monocode:project-chat-backgrounds' || $key eq 'monocode.keybindingOverrides') {
        return $raw_bytes unless ref($data) eq 'HASH';
        my $changed = 0;
        for my $old (keys %$data) {
            my $new = $old;
            $new =~ s/^mono:/bot:/ if $key eq 'monocode:project-chat-backgrounds';
            $new = 'App: Toggle Bot' if $key eq 'monocode.keybindingOverrides' && $old eq 'App: Toggle Mono';
            next if $old eq $new;
            $data->{$new} = $data->{$old} unless exists $data->{$new};
            delete $data->{$old};
            $changed = 1;
        }
        return $raw_bytes unless $changed;
    } else {
        return $raw_bytes;
    }

    my $new_json = eval { JSON::PP->new->utf8(0)->encode($data) };
    return $raw_bytes unless defined $new_json;

    return encode("UTF-16LE", $new_json);
}

sub apply_webkit_db {
    my ($file) = @_;
    my @statements;
    for my $copy (localstorage_migrations($file)) {
        my ($old_k, $new_k, $old_hex, $dest_hex) = @$copy;
        my $raw_val = pack("H*", $old_hex);
        my $new_val = transform_localstorage_value($old_k, $raw_val);
        my $new_hex = unpack("H*", $new_val);
        my $esc_k = $new_k;
        $esc_k =~ s/'/''/g;
        if (defined $dest_hex) {
            push @statements, "UPDATE ItemTable SET value = X'$new_hex' WHERE key = '$esc_k' AND hex(value) = '$dest_hex';";
        } else {
            push @statements, "INSERT OR IGNORE INTO ItemTable (key, value) VALUES ('$esc_k', X'$new_hex');";
        }
    }

    if (@statements) {
        open my $in, "|-", "/usr/bin/sqlite3", "-batch", "-bail", $file or die "Failed to open sqlite3: $!\n";
        print $in "BEGIN IMMEDIATE;\n";
        print $in join("\n", @statements) . "\n";
        print $in "COMMIT;\n";
        close $in or die "Failed to apply webkit updates: $!\n";
    }
}

sub transform_block_data {
    my ($data) = @_;
    if (ref($data) eq 'HASH') {
        for my $pair (
            ['monoHabit', 'botHabit'],
            ['monoCard', 'botCard'],
            ['monoReaction', 'botReaction'],
            ['monoSessionCompletion', 'botSessionCompletion'],
            ['monoPeerMessage', 'botPeerMessage'],
            ['monoSpawnedSessions', 'botSpawnedSessions'],
        ) {
            my ($old, $new) = @$pair;
            if (exists $data->{$old} && !exists $data->{$new}) {
                $data->{$new} = delete $data->{$old};
            }
        }
        if (exists $data->{botPeerMessage} && ref($data->{botPeerMessage}) eq 'HASH') {
            my $p = $data->{botPeerMessage};
            $p->{fromBotId} = delete $p->{fromMonoId} if exists $p->{fromMonoId} && !exists $p->{fromBotId};
            $p->{toBotId} = delete $p->{toMonoId} if exists $p->{toMonoId} && !exists $p->{toBotId};
        }
        $data->{fromBotId} = delete $data->{fromMonoId} if exists $data->{fromMonoId} && !exists $data->{fromBotId};
        $data->{toBotId} = delete $data->{toMonoId} if exists $data->{toMonoId} && !exists $data->{toBotId};
        for my $v (values %$data) {
            transform_block_data($v);
        }
    } elsif (ref($data) eq 'ARRAY') {
        for my $item (@$data) {
            transform_block_data($item);
        }
    }
}

sub pending_transcripts_query {
    my ($db) = @_;
    return undef unless table_exists($db, 'mono_transcripts');
    my @empty_destination;
    my @legacy_content;
    for my $suffix (qw(blocks turns)) {
        if (table_exists($db, "bot_$suffix")) {
            push @empty_destination, "NOT EXISTS (SELECT 1 FROM bot_$suffix WHERE session_id = legacy.session_id)";
        }
        if (table_exists($db, "mono_$suffix")) {
            push @legacy_content, "EXISTS (SELECT 1 FROM mono_$suffix WHERE session_id = legacy.session_id)";
        }
    }
    if (table_exists($db, 'bot_transcripts')) {
        # A marker alone can come from opening a session whose blocks_json was
        # already cleared. Recover only if BOTH destination indexes are empty;
        # populated or partial Bot history is ambiguous and must stay intact.
        my $has_legacy_content = @legacy_content ? join(" OR ", @legacy_content) : '0';
        push @empty_destination, "(NOT EXISTS (SELECT 1 FROM bot_transcripts WHERE session_id = legacy.session_id) OR $has_legacy_content)";
    }
    return "SELECT legacy.session_id FROM mono_transcripts legacy" . (@empty_destination ? " WHERE " . join(" AND ", @empty_destination) : "");
}

sub check_monocode_db_pending {
    my ($db) = @_;
    return 0 unless is_sqlite3($db);
    if (table_exists($db, 'schema_migrations')) {
        open my $fh, "-|", "/usr/bin/sqlite3", "-batch", $db, "SELECT 1 FROM schema_migrations WHERE version=22;" or die $!;
        my $done = <$fh>;
        close $fh or die "Failed to read migration state\n";
        return 0 if $done;
    }

    my $transcripts = pending_transcripts_query($db);
    if (defined $transcripts) {
        open my $ch, "-|", "/usr/bin/sqlite3", "-batch", $db, "$transcripts LIMIT 1;" or return 0;
        my $unmigrated = <$ch>; close $ch;
        return 1 if $unmigrated;
    }

    my @patterns = qw(monoHabit monoCard monoReaction monoSessionCompletion monoPeerMessage monoSpawnedSessions);

    if (table_exists($db, 'sessions')) {
        open my $ch, "-|", "/usr/bin/sqlite3", "-batch", "-tabs", $db, "PRAGMA table_info(sessions);" or return 0;
        my %has_col;
        while (<$ch>) { my @p = split /\t/; $has_col{$p[1]} = 1 if @p > 1; }
        close $ch;
        my @conds;
        for my $col (qw(blocks_json queued_messages_json)) {
            if ($has_col{$col}) {
                push @conds, map { "$col LIKE '%$_%'" } @patterns;
            }
        }
        if (@conds) {
            my $sql = "SELECT 1 FROM sessions WHERE (" . join(" OR ", @conds) . ") LIMIT 1;";
            open my $qh, "-|", "/usr/bin/sqlite3", "-batch", $db, $sql or return 0;
            my $found = <$qh>; close $qh;
            return 1 if $found;
        }
    }

    if (table_exists($db, 'ordinary_blocks')) {
        my $cond = join(" OR ", map { "block_json LIKE '%$_%'" } @patterns);
        open my $qh, "-|", "/usr/bin/sqlite3", "-batch", $db, "SELECT 1 FROM ordinary_blocks WHERE ($cond) LIMIT 1;" or return 0;
        my $found = <$qh>; close $qh;
        return 1 if $found;
    }

    if (table_exists($db, 'local_submissions')) {
        my $cond = join(" OR ", map { "user_turn_json LIKE '%$_%'" } @patterns);
        open my $qh, "-|", "/usr/bin/sqlite3", "-batch", $db, "SELECT 1 FROM local_submissions WHERE ($cond) LIMIT 1;" or return 0;
        my $found = <$qh>; close $qh;
        return 1 if $found;
    }

    if (table_exists($db, 'workspace_snapshot')) {
        open my $qh, "-|", "/usr/bin/sqlite3", "-batch", $db, "SELECT 1 FROM workspace_snapshot WHERE snapshot_json LIKE '%\"mono:%' LIMIT 1;" or return 0;
        my $found = <$qh>; close $qh;
        return 1 if $found;
    }

    return 0;
}

sub apply_monocode_db {
    my ($db) = @_;
    return unless is_sqlite3($db);
    return unless check_monocode_db_pending($db);

    my @sql = ("BEGIN IMMEDIATE;");
    push @sql, q{
CREATE TABLE IF NOT EXISTS bot_transcripts (
  session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS bot_turns (
  session_id TEXT NOT NULL REFERENCES bot_transcripts(session_id) ON DELETE CASCADE,
  seq INTEGER NOT NULL,
  PRIMARY KEY (session_id, seq)
);
CREATE TABLE IF NOT EXISTS bot_blocks (
  session_id TEXT NOT NULL REFERENCES bot_transcripts(session_id) ON DELETE CASCADE,
  seq INTEGER NOT NULL,
  block_id TEXT NOT NULL,
  block_json TEXT NOT NULL,
  search_text TEXT NOT NULL,
  is_user INTEGER NOT NULL,
  is_draft INTEGER NOT NULL,
  PRIMARY KEY (session_id, seq),
  UNIQUE (session_id, block_id)
);
CREATE INDEX IF NOT EXISTS bot_blocks_user ON bot_blocks(session_id, is_user, is_draft);
};

    my $q = pending_transcripts_query($db);
    if (defined $q) {
        open my $p, "-|", "/usr/bin/sqlite3", "-batch", "-tabs", $db, $q or die "Failed to query mono_transcripts: $!\n";
        my @sids;
        while (<$p>) { chomp; push @sids, $_ if length $_; }
        close $p;

        my $has_turns = table_exists($db, 'mono_turns');
        my $has_blocks = table_exists($db, 'mono_blocks');
        my %session_columns;
        open my $columns, "-|", "/usr/bin/sqlite3", "-batch", "-tabs", $db, "PRAGMA table_info(sessions);" or die $!;
        while (<$columns>) { my @p = split /\t/; $session_columns{$p[1]} = 1 if @p > 1; }
        close $columns or die "Failed to read session schema\n";

        for my $sid (@sids) {
            my $h_sid = unpack("H*", encode("UTF-8", $sid));
            push @sql, "INSERT OR IGNORE INTO bot_transcripts (session_id) VALUES (CAST(X'$h_sid' AS TEXT));";
            if ($has_turns) {
                push @sql, "INSERT OR IGNORE INTO bot_turns (session_id, seq) SELECT session_id, seq FROM mono_turns WHERE session_id = CAST(X'$h_sid' AS TEXT);";
            }
            if ($has_blocks) {
                open my $bp, "-|", "/usr/bin/sqlite3", "-batch", "-tabs", $db, "SELECT seq, hex(block_id), hex(block_json), hex(search_text), is_user, is_draft FROM mono_blocks WHERE session_id = CAST(X'$h_sid' AS TEXT) ORDER BY seq;" or die "Failed to read mono_blocks: $!\n";
                while (<$bp>) {
                    chomp;
                    my ($seq, $h_bid, $h_bjson, $h_stext, $is_user, $is_draft) = split /\t/;
                    my $raw_json = decode("UTF-8", pack("H*", $h_bjson // ""));
                    my $data = eval { JSON::PP->new->utf8(0)->decode($raw_json) };
                    my $h_new_json = $h_bjson;
                    if ($data) {
                        transform_block_data($data);
                        my $new_json = JSON::PP->new->utf8(0)->encode($data);
                        $h_new_json = unpack("H*", encode("UTF-8", $new_json));
                    }
                    push @sql, "INSERT OR IGNORE INTO bot_blocks (session_id, seq, block_id, block_json, search_text, is_user, is_draft) VALUES (CAST(X'$h_sid' AS TEXT), $seq, CAST(X'$h_bid' AS TEXT), CAST(X'$h_new_json' AS TEXT), CAST(X'$h_stext' AS TEXT), $is_user, $is_draft);";
                }
                close $bp;
            }
            my @flags;
            push @flags, "has_user_message = EXISTS(SELECT 1 FROM bot_blocks WHERE session_id = CAST(X'$h_sid' AS TEXT) AND is_user = 1)" if $session_columns{has_user_message};
            push @flags, "is_draft = EXISTS(SELECT 1 FROM bot_blocks WHERE session_id = CAST(X'$h_sid' AS TEXT) AND is_user = 1 AND is_draft = 1)" if $session_columns{is_draft};
            push @sql, "UPDATE sessions SET " . join(", ", @flags) . " WHERE id = CAST(X'$h_sid' AS TEXT);" if @flags;
        }
    }

    if (table_exists($db, 'sessions')) {
        open my $ch, "-|", "/usr/bin/sqlite3", "-batch", "-tabs", $db, "PRAGMA table_info(sessions);" or die $!;
        my %has_col;
        while (<$ch>) { my @p = split /\t/; $has_col{$p[1]} = 1 if @p > 1; }
        close $ch;

        my $has_b = $has_col{blocks_json};
        my $has_q = $has_col{queued_messages_json};
        if ($has_b || $has_q) {
            my @cols = ("hex(id)");
            push @cols, $has_b ? "hex(blocks_json)" : "''";
            push @cols, $has_q ? "hex(queued_messages_json)" : "''";
            my $q = "SELECT " . join(", ", @cols) . " FROM sessions;";
            open my $sp, "-|", "/usr/bin/sqlite3", "-batch", "-tabs", $db, $q or die "Failed to read sessions: $!\n";
            while (<$sp>) {
                chomp;
                my ($h_id, $h_bjson, $h_qjson) = split /\t/;
                my $bjson = decode("UTF-8", pack("H*", $h_bjson // ""));
                my $qjson = decode("UTF-8", pack("H*", $h_qjson // ""));
                my ($changed, $new_h_bjson, $new_h_qjson) = (0, $h_bjson, $h_qjson);
                if ($has_b && length $bjson) {
                    my $d = eval { JSON::PP->new->utf8(0)->decode($bjson) };
                    if ($d) {
                        transform_block_data($d);
                        my $nj = JSON::PP->new->utf8(0)->encode($d);
                        $new_h_bjson = unpack("H*", encode("UTF-8", $nj));
                        $changed = 1;
                    }
                }
                if ($has_q && length $qjson) {
                    my $d = eval { JSON::PP->new->utf8(0)->decode($qjson) };
                    if ($d) {
                        transform_block_data($d);
                        my $nj = JSON::PP->new->utf8(0)->encode($d);
                        $new_h_qjson = unpack("H*", encode("UTF-8", $nj));
                        $changed = 1;
                    }
                }
                if ($changed) {
                    my @updates;
                    push @updates, "blocks_json = CAST(X'$new_h_bjson' AS TEXT)" if $has_b;
                    push @updates, "queued_messages_json = CAST(X'$new_h_qjson' AS TEXT)" if $has_q;
                    push @sql, "UPDATE sessions SET " . join(", ", @updates) . " WHERE id = CAST(X'$h_id' AS TEXT);";
                }
            }
            close $sp;
        }
    }

    if (table_exists($db, 'ordinary_blocks')) {
        my @patterns = qw(monoHabit monoCard monoReaction monoSessionCompletion monoPeerMessage monoSpawnedSessions);
        my $cond = join(" OR ", map { "block_json LIKE '%$_%'" } @patterns);
        open my $op, "-|", "/usr/bin/sqlite3", "-batch", "-tabs", $db, "SELECT rowid, hex(block_json) FROM ordinary_blocks WHERE ($cond);" or die $!;
        while (<$op>) {
            chomp;
            my ($rowid, $h_bjson) = split /\t/;
            my $bjson = decode("UTF-8", pack("H*", $h_bjson // ""));
            my $d = eval { JSON::PP->new->utf8(0)->decode($bjson) };
            if ($d) {
                transform_block_data($d);
                my $nj = JSON::PP->new->utf8(0)->encode($d);
                my $new_h = unpack("H*", encode("UTF-8", $nj));
                push @sql, "UPDATE ordinary_blocks SET block_json = CAST(X'$new_h' AS TEXT) WHERE rowid = $rowid;";
            }
        }
        close $op;
    }

    if (table_exists($db, 'local_submissions')) {
        my @patterns = qw(monoHabit monoCard monoReaction monoSessionCompletion monoPeerMessage monoSpawnedSessions);
        my $cond = join(" OR ", map { "user_turn_json LIKE '%$_%'" } @patterns);
        open my $lp, "-|", "/usr/bin/sqlite3", "-batch", "-tabs", $db, "SELECT rowid, hex(user_turn_json) FROM local_submissions WHERE ($cond);" or die $!;
        while (<$lp>) {
            chomp;
            my ($rowid, $h_tjson) = split /\t/;
            my $tjson = decode("UTF-8", pack("H*", $h_tjson // ""));
            my $d = eval { JSON::PP->new->utf8(0)->decode($tjson) };
            if ($d) {
                transform_block_data($d);
                my $nj = JSON::PP->new->utf8(0)->encode($d);
                my $new_h = unpack("H*", encode("UTF-8", $nj));
                push @sql, "UPDATE local_submissions SET user_turn_json = CAST(X'$new_h' AS TEXT) WHERE rowid = $rowid;";
            }
        }
        close $lp;
    }

    if (table_exists($db, 'workspace_snapshot')) {
        open my $wp, "-|", "/usr/bin/sqlite3", "-batch", "-tabs", $db, "SELECT rowid, hex(snapshot_json) FROM workspace_snapshot WHERE snapshot_json LIKE '%\"mono:%';" or die $!;
        while (<$wp>) {
            chomp;
            my ($rowid, $h_snap) = split /\t/;
            my $snap = decode("UTF-8", pack("H*", $h_snap // ""));
            $snap =~ s/\"mono:([^\"]+)\"/\"bot:$1\"/g;
            my $new_h_snap = unpack("H*", encode("UTF-8", $snap));
            push @sql, "UPDATE workspace_snapshot SET snapshot_json = CAST(X'$new_h_snap' AS TEXT) WHERE rowid = $rowid;";
        }
        close $wp;
    }

    push @sql, "COMMIT;";

    open my $in, "|-", "/usr/bin/sqlite3", "-batch", "-bail", $db or die "Failed to open sqlite3 for writing: $!\n";
    print $in join("\n", @sql) . "\n";
    close $in or die "SQLite transaction failed: $!\n";
}

my ($action, $target) = @ARGV;
if ($action eq 'check-localstorage') {
    my $found = 0;
    for my $db (find_webkit_dbs($target)) {
        if (webkit_db_has_unmigrated($db)) {
            $found = 1;
            last;
        }
    }
    print "$found\n";
} elsif ($action eq 'apply-localstorage') {
    for my $db (find_webkit_dbs($target)) {
        apply_webkit_db($db);
    }
} elsif ($action eq 'check-sqlite') {
    my $found = check_monocode_db_pending($target);
    print "$found\n";
} elsif ($action eq 'apply-sqlite') {
    apply_monocode_db($target);
} else {
    die "Unknown action: $action\n";
}
PERL
}
legacy_dir_pending() {
  local source=$1 target=$2 child
  [ -d "$source" ] && [ ! -L "$source" ] || return 1
  [ -e "$target" ] || [ -L "$target" ] || return 0
  [ -d "$target" ] && [ ! -L "$target" ] || return 1
  for child in "$source"/* "$source"/.[!.]* "$source"/..?*; do
    [ -e "$target/$(basename "$child")" ] || [ -L "$target/$(basename "$child")" ] || return 0
  done
  return 1
}

# A missing owner is published as one complete unit. Never mix legacy files
# into an owner that already exists, including an explicitly empty owner.
copy_legacy_dir() {
  local source=$1 target=$2 child payload target_child
  [ -d "$source" ] && [ ! -L "$source" ] || return 0
  if [ ! -e "$target" ] && [ ! -L "$target" ]; then
    staging_legacy=$(mktemp -d "$(dirname "$target")/.vorkspace-$(basename "$target").XXXXXXXX")
    ditto "$source" "$staging_legacy"
    chmod 700 "$staging_legacy"
    apps_closed
    /usr/bin/perl -e '(-e $ARGV[1] || -l $ARGV[1]) and die "Bots destination changed; refusing to overwrite\n"; rename $ARGV[0], $ARGV[1] or die "Bots destination changed; refusing to overwrite: $!\n"' -- "$staging_legacy" "$target"
    staging_legacy=''
  elif [ -d "$target" ] && [ ! -L "$target" ]; then
    for child in "$source"/* "$source"/.[!.]* "$source"/..?*; do
      target_child="$target/$(basename "$child")"
      [ ! -e "$target_child" ] && [ ! -L "$target_child" ] || continue
      staging_legacy=$(mktemp -d "$(dirname "$target")/.vorkspace-$(basename "$target").XXXXXXXX")
      payload="$staging_legacy/payload"
      if [ -L "$child" ]; then cp -pP "$child" "$payload"
      else ditto "$child" "$payload"; fi
      apps_closed
      /usr/bin/perl -e '
        (-e $ARGV[1] || -l $ARGV[1]) and die "Bots destination changed; refusing to overwrite\n";
        if (-d $ARGV[0] && !-l $ARGV[0]) { rename $ARGV[0], $ARGV[1] or die "Bots destination changed: $!\n"; }
        else { link $ARGV[0], $ARGV[1] or die "Bots destination changed: $!\n"; }
      ' -- "$payload" "$target_child"
      rm -rf "$staging_legacy"
      staging_legacy=''
    done
  fi
}
legacy_copies=()
for pair in 'monos:bots' 'codex-monos:codex-bots'; do
  src_dir="${pair%%:*}"
  dst_dir="${pair##*:}"
  if [ -n "$source_profile" ]; then
    if legacy_dir_pending "$source_profile/$src_dir" "$source_profile/$dst_dir"; then
      legacy_copies+=("$pair")
    fi
  else
    profile="$home/Library/Application Support/com.monocode.desktop"
    if legacy_dir_pending "$profile/$src_dir" "$profile/$dst_dir"; then
      legacy_copies+=("$pair")
    fi
  fi
done
if [ -n "$source_profile" ]; then
  webkit_check_dir="$source_profile"
  db_check_file="$source_profile/monocode.db"
else
  webkit_check_dir="$home/Library/WebKit/com.monocode.desktop"
  db_check_file="$home/Library/Application Support/com.monocode.desktop/monocode.db"
fi
legacy_localstorage=0
legacy_sqlite=0
if [ -x /usr/bin/perl ] && [ -x /usr/bin/sqlite3 ]; then
  if [ -d "$webkit_check_dir" ]; then
    legacy_localstorage=$(run_migration_perl check-localstorage "$webkit_check_dir")
  fi
  if [ -f "$db_check_file" ]; then
    legacy_sqlite=$(run_migration_perl check-sqlite "$db_check_file")
  fi
fi
printf 'vorkspace reuses the existing MonoCode profile (com.monocode.desktop).\n'
printf 'App-owned locations found: %s\nBackup: %s\n' "${#paths[@]}" "$backup"
for relative in "${paths[@]}"; do printf '  %s\n' "$relative"; done
[ -z "$source_profile" ] || printf 'Recovery profile: %s -> %s\n' "$source_profile" "$destination"
for pair in "${legacy_copies[@]}"; do
  src_dir="${pair%%:*}"
  dst_dir="${pair##*:}"
  printf 'Legacy %s copy: %s -> %s\n' "$src_dir" "$src_dir" "$dst_dir"
done
if [ "$legacy_localstorage" -eq 1 ]; then
  printf 'Legacy localStorage migration: monocode:mono-* -> monocode:bot-*\n'
fi
if [ "$legacy_sqlite" -eq 1 ]; then
  printf 'Legacy SQLite migration: mono_* -> bot_*\n'
fi
if [ "$apply" -eq 0 ]; then printf 'Dry-run only. Use --apply after closing the apps.\n'; exit 0; fi
# Check process names without printing command lines, tokens, or environment.
closed() {
  if pgrep "$@" >/dev/null; then fail 'Close MonoCode, vorkspace and the local host before applying.'
  else [ "$?" -eq 1 ] || fail 'Unable to check running apps; refusing to copy live data.'; fi
}
apps_closed() {
  closed -x 'monocode|MonoCode|vorkspace'
  closed -f '(build/host/(monocode|vorkspace)-host[.]mjs|/[.]monocode-host/.*host[.]mjs).*serve'
}
apps_closed
[ -z "$source_profile" ] || [ -x /usr/bin/perl ] || fail 'Recovery requires macOS /usr/bin/perl.'
[ "${#legacy_copies[@]}" -eq 0 ] || [ -x /usr/bin/perl ] || fail 'Migration requires macOS /usr/bin/perl.'
[ "$legacy_localstorage" -eq 0 ] || [ -x /usr/bin/perl ] || fail 'Migration requires macOS /usr/bin/perl.'
[ "$legacy_sqlite" -eq 0 ] || [ -x /usr/bin/perl ] || fail 'Migration requires macOS /usr/bin/perl.'
[ "$legacy_localstorage" -eq 0 ] || [ -x /usr/bin/sqlite3 ] || fail 'Migration requires macOS /usr/bin/sqlite3.'
[ "$legacy_sqlite" -eq 0 ] || [ -x /usr/bin/sqlite3 ] || fail 'Migration requires macOS /usr/bin/sqlite3.'
[ "${#paths[@]}" -gt 0 ] || [ -n "$source_profile" ] || fail 'No app-owned data found; nothing to copy.'
mkdir -p "$(dirname "$backup")"
mkdir -m 700 "$backup"
for relative in "${paths[@]}"; do
  mkdir -p "$(dirname "$backup/$relative")"
  if [ -L "$home/$relative" ]; then cp -pP "$home/$relative" "$backup/$relative"
  else ditto "$home/$relative" "$backup/$relative"; fi
  printf '%s\n' "$relative" >> "$backup/manifest.txt"
done
staging_legacy=''
staging=''
cleanup() {
  [ -z "$staging_legacy" ] || rm -rf "$staging_legacy"
  [ -z "$staging" ] || rm -rf "$staging"
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM
if [ -z "$source_profile" ]; then
  profile="$home/Library/Application Support/com.monocode.desktop"
  for pair in 'monos:bots' 'codex-monos:codex-bots'; do
    src_dir="${pair%%:*}"
    dst_dir="${pair##*:}"
    copy_legacy_dir "$profile/$src_dir" "$profile/$dst_dir"
  done
  if [ "$legacy_localstorage" -eq 1 ]; then
    apps_closed
    run_migration_perl apply-localstorage "$home/Library/WebKit/com.monocode.desktop"
  fi
  if [ "$legacy_sqlite" -eq 1 ]; then
    apps_closed
    run_migration_perl apply-sqlite "$profile/monocode.db"
  fi
fi
if [ -n "$source_profile" ]; then
  # Back up the complete recovery input before publishing the requested copy.
  ditto "$source_profile" "$backup/recovery-source"
  mkdir -p "$(dirname "$destination")"
  staging=$(mktemp -d "$(dirname "$destination")/.vorkspace-recovery.XXXXXXXX")
  ditto "$backup/recovery-source" "$staging"
  chmod 700 "$staging"
  for pair in 'monos:bots' 'codex-monos:codex-bots'; do
    src_dir="${pair%%:*}"
    dst_dir="${pair##*:}"
    copy_legacy_dir "$staging/$src_dir" "$staging/$dst_dir"
  done
  if [ "$legacy_localstorage" -eq 1 ]; then
    run_migration_perl apply-localstorage "$staging"
  fi
  if [ "$legacy_sqlite" -eq 1 ]; then
    run_migration_perl apply-sqlite "$staging/monocode.db"
  fi
  apps_closed
  # POSIX directory rename refuses a now-populated directory or symlink target.
  /usr/bin/perl -e 'rename $ARGV[0], $ARGV[1] or die "Recovery destination changed; refusing to overwrite: $!\n"' -- "$staging" "$destination"
  staging=''
fi
printf 'Backup complete: %s\nOriginal files and Keychain entries remain in place.\n' "$backup"
