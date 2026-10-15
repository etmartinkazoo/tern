# tern

A dense, vim-driven mail client for the terminal that wears your Omarchy theme.
Website and docs: **https://ternmail.dev**

```
 tern you@x.com         │ Inbox  412 · 6 unread                  │ Re: Q4 roadmap — final cut before Friday
▎ Inbox              6  │ ●  Maya Okafor   Re: Q4 roadmap…  03:45 │ From    Maya Okafor <maya@northwind.io>
  Drafts                │    GitHub        [omarchy] feat…  03:22 │ To      Ted Martin <ted@example.com>
  Sent                  │ ●  Priya Raman   Contract draft…    Tue │ Date    Wed 14 Oct 2026 03:45
  Archive               │                                         │ ──────────────────────────────── all
  Trash                 │                                         │ Hey,
```

Sidebar · message list (40%) · message (60%). Mail (IMAP + SMTP, with push), a
multi-account calendar (CalDAV) and contacts (CardDAV). Google and Microsoft sign
in with OAuth.

Built for getting through a day of business and personal mail:

- **One inbox for every account** (`gu`), and replies go out from the account
  they came to. `^f` in the composer switches who it's from.
- **Nothing gets lost.** Sending goes through an outbox: `^z` takes it back for
  a few seconds, and a send that fails offline is retried until it goes. What
  you're writing is autosaved and survives a crash.
- **Snooze** (`b`) hides a message until tomorrow morning, Monday or `3h` from
  now, then puts it back unread.
- **Mail and calendar together.** Today's meetings sit under your folders,
  invitations are answered from the message, `ge` turns an email into an event,
  and `gm` / `gz` add a Meet or Zoom link.
- **People** (`P`): each contact with your recent mail and upcoming meetings.
- **Notifications** for new mail and for meetings about to start.
- **`tern doctor`** checks every account and server and tells you what's wrong.

The protocol work is done by [Pimalaya](https://github.com/pimalaya)'s libraries:
`io-imap` (IMAP, IDLE), `io-webdav` (CalDAV/CardDAV discovery and sync),
`ical-rs` (iCalendar, recurrence, time zones) and `io-oauth` (sign-in). tern
itself is the interface.

## Install

On Debian, Ubuntu or Fedora:

```sh
curl -fsSL https://raw.githubusercontent.com/etmartinkazoo/tern/main/install.sh | sh
```

The script installs a C compiler, git and the helper tools tern calls
(`notify-send`, `wl-copy`, `xdg-open`) with `apt` or `dnf`, using `sudo` when
you aren't root. If there's no Rust 1.89 or newer on your `PATH` (Ubuntu's own
is too old), it installs one with rustup into `~/.cargo`. Then it builds tern
and puts it in `~/.cargo/bin`. If that isn't on your `PATH` but `~/.local/bin`
is, it links `tern` there; otherwise it tells you what to add. Set `TERN_REF=<tag or branch>` to build
something other than `main`, or `TERN_NO_EXTRAS=1` to skip the helper tools.
Run it again to update.

Anywhere else, with Rust 1.89+ and a C compiler:

```sh
cargo install --locked --git https://github.com/etmartinkazoo/tern   # or, in a checkout: cargo install --path .
```

## Run it

```sh
tern --demo                # look around with a fake mailbox
tern --init                # writes ~/.config/tern/config.toml
tern doctor                # check every account and server
tern
```

`tern doctor` signs in to each IMAP and SMTP server (it sends nothing), checks
password commands and OAuth tokens, finds the calendars and address books on
each `[[dav]]` server, and looks at the theme, time zone, editor, helper tools
and anything stuck in the outbox. Each problem comes with what to do about it,
and it exits 1 when something fails.

## Omarchy

tern reads the active theme from `~/.config/omarchy/current/theme`. It looks for
`colors.toml`, then `alacritty.toml`, `kitty.conf`, and `ghostty.conf`. When
`omarchy-theme-set` repoints the symlink, tern picks up the new theme within
about two seconds, without a restart. Glyphs come from a Nerd Font, which
Omarchy ships by default (set `icons = false` to turn them off). tern leaves
the terminal's background alone, so transparency still works
(`paint_background = true` overrides that).

Other locations: `TERN_THEME_DIR=…` or `[ui] theme_dir`. If no theme is found,
tern falls back to a built-in Tokyo Night.

## Keys

| move | | triage | |
|---|---|---|---|
| `j` `k` | down / up (counts work: `5j`) | `e` | archive |
| `h` `l` / `⏎` | focus left / right | `d` | trash (purge when already in Trash) |
| `J` `K` | next / prev message from any pane | `!` | junk |
| `gg` `G` | top / bottom | `M` | move to folder (fuzzy) |
| `^d` `^u` `^f` `^b` | half / full page | `s` | flag |
| `space` | page the message, then go to the next one | `u` | read / unread |
| `zz` | zoom the message full-screen | `x` / `X` | mark / mark all; actions apply to marks |
| `gi gs gd ga gt gj gf` | inbox, sent, drafts, archive, trash, junk, flagged | `/` | live filter; `⏎` also searches the server |
| `gu` | all inboxes, every account in one list | `b` | snooze until… |
| `$` | check for mail now | `esc` | clear filter / marks / zoom |
| `l` `h` | on a conversation: open it into its messages / fold it back | | |
| `za` `zo` `zc` | toggle / open / fold a conversation | `zM` `zR` | fold / unfold all |
| `T` | threads on / off | | |

| write | |
|---|---|
| `m` | new message |
| `r` `R` `f` | reply / reply all / forward (forward carries attachments) |
| `H` | show the message as HTML / as text |
| `I` | load remote pictures in the message |
| `o` | open the message in the browser (HTML, or the text if there's none) |
| `L` `gx` | links in the message: type to filter, `⏎` opens, `^y` copies |
| `S` | save attachments to `download_dir` |
| `U` | follow List-Unsubscribe |
| `A` | add the sender to CardDAV |
| `y` | copy the sender's address (`wl-copy`) |
| `gc` | sync contacts now |
| `ge` | make a calendar event from the message |
| `gr` | resume the message you were writing last time |
| `go` | outbox: what's waiting to send, and what failed |
| `^z` | undo the send you just made |
| `P` | people |
| `C` | calendar |

### Composing

Messages are written in the right-hand column with the list still in view. Fields
are To, Cc, Subject (Bcc appears when you tab to it) and the body. The body is
modal like vim. Long lines wrap on screen only; the message goes out with one
line per paragraph, so phones and desktop clients each wrap it to fit.

| insert | |
|---|---|
| `tab` | take the contact suggestion, or go to the next field |
| `⏎` | take the suggestion, or jump To → Subject → body |
| `^n` `^p` | pick a suggestion |
| `esc` | normal mode |

| normal | |
|---|---|
| `hjkl` `w b e` `0 $` `gg G` `{ }` | move (`j`/`k` cross between fields) |
| `i a I A o O` | insert |
| `x dd dw D cc cw C S r J` | edit |
| `yy p P` `v V` | yank, paste, visual |
| `u` `^r` | undo / redo |
| `ZZ` `:wq` `^s` | send |
| `:w` | save to Drafts |
| `ZQ` `:q!` | discard (`:q` refuses if there are unsent changes) |
| `^t` `:a <file>` | attach |
| `:e` | edit in `$EDITOR` full screen, then come back |
| `^f` | send from your next account (its signature replaces the old one) |

## Sending, and not losing anything

A sent message goes to the outbox in `~/.local/share/tern/outbox/` before
anything else happens. It waits `undo_send_secs` (10 by default) so `^z` can
bring it back into the composer, then goes out. If sending fails (offline, the
server is down), it stays queued and is retried after 30s, 2m, 10m and then
every 30 minutes, including after a restart. The status bar shows how many are
waiting. `go` opens the outbox: `r` retries now, `⏎` edits, `d` discards.

While you write, the composer is saved to `~/.local/share/tern/draft.json`
every few seconds. If tern closes before you send, the next start tells you,
and `gr` picks it up where you left off. `:w` saves to the server's Drafts
folder, and `⏎` on a message in Drafts opens it for editing; sending it
removes the draft.

## All inboxes

With more than one account, `gu` (or "All inboxes" at the top of the sidebar)
shows every account's inbox as one list, newest first, with each message
tagged by account. Everything works as usual: archive, trash and move go to the
right account's folders, and a reply is sent from the account the message came
to. `^f` in the composer cycles the From address through your accounts and
swaps the signature to match.

## Snooze

`b` asks until when: pick later today, tomorrow morning, this weekend or next
week, or type `3h`, `2d`, `tomorrow 9am`, `fri 2pm`. The message moves to a
`Snoozed` folder on its server (created the first time), so it's out of your
inbox on your phone too. When the time comes, and tern is running, it's moved
back to the folder it came from, marked unread, with a notification. Snoozes are remembered
in `~/.local/share/tern/snoozed.json` and caught up on the next start.

## People

`P` opens your contacts from every CardDAV address book. The right pane shows
the selected person's addresses, phone and notes, your upcoming meetings with
them, and recent mail from the loaded folders. `m` or `⏎` writes to them, `a`
adds a contact, `e` edits, `d` deletes (asks first), `y` copies the address and
`/` filters. Changes are saved to the server. `A` on a message adds its sender.

## Notifications and today

New mail in your inboxes and upcoming meetings show as desktop notifications
(`notify-send`, which Omarchy's mako displays). A meeting reminds you at its own
alarm, or `reminder_minutes` before (10 by default). The mail sidebar shows
today's remaining meetings under your folders; `agenda_in_mail = false` hides
them. Both can be turned off under Settings → General.

## Accounts

Settings → Accounts (`,` then `2`) lists your accounts. `a` adds one: pick
Fastmail, Google, iCloud, Microsoft 365 or Other, and the form comes prefilled
with that provider's servers, including the calendar and contacts servers, so one
form sets up mail, calendar and contacts together. `⏎` edits an account and `d`
removes it. Saving writes `config.toml` in place (your comments stay) and
connects the account right away. Server fields take a port and, if the port
doesn't imply it, a scheme: `imap://host:3143` for no encryption,
`starttls://host:2525`.

Settings → Calendars (`,` then `3`) manages calendar and contacts servers,
including the ones created with an account. Each server shows the calendars
found on it. On a calendar, `space` shows or hides it and `*` makes it the
default for new events and accepted invitations. On a server, `⏎` edits it and
`d` removes it. `a` adds a server (Fastmail, iCloud, Google, Nextcloud or any
CalDAV/CardDAV URL), either with its own password command or reusing a mail
account's sign-in. `$` syncs. Hidden calendars and the default are saved in
`config.toml`, so the sidebar's `space` toggle is remembered too.

With more than one account, each one is a fold in the sidebar: `⏎`, `space`,
`za` or a click on its heading opens or closes it, and `h` on a folder closes its
account. A closed account shows its inbox's unread count. Folds are remembered.

## Calendar

`C` switches between mail and the calendar. The calendar uses the same three
panes: your calendars on the left with a small month view, an agenda in the
middle, and the selected event on the right. Recurring events (with skipped and
moved instances), all-day and multi-day events, and time zones are handled. Each
calendar keeps the color the server gives it.

| | |
|---|---|
| `j` `k` | next / previous event |
| `h` `l` · `H` `L` · `[` `]` | day · week · month |
| `t` | today |
| `space` (sidebar) | show / hide a calendar |
| `/` | filter events |
| `o` | open the event's link (video call, etc.) |
| `a` | quick add: `Lunch with Maya fri 12:30 1h @Osteria` |
| `A` · `e` | new event / edit the selected one (form in the right pane) |
| `d` | delete (asks first; a recurring event goes as a series) |
| `$` | sync now (it also syncs every 15 minutes) |

Add one `[[dav]]` block per server. tern finds every calendar and address book
on that server:

```toml
[[dav]]
name = "Fastmail"
url = "https://caldav.fastmail.com/"
username = "you@example.com"
password_cmd = "pass show mail/personal"   # or: account = "Personal" to reuse its sign-in
```

Syncing only downloads what changed since the last sync when the server
supports that, and the result is cached, so the calendar opens instantly and
works offline.

Calendar times are shown and entered in one time zone, US Eastern
(`America/New_York`) by default. Change it under Settings → General → Time zone
with an IANA name or `est`/`cst`/`mst`/`pst`/`utc`; daylight time is handled.
The agenda opens on today, with earlier days a `k` away. Edits change only the title, times, place and notes, and keep
everything else in the event (attendees, alarms, time zone) as it was.

**Call links.** `gm` on an event creates a new Google Meet link; `gz` adds your
personal Zoom link (Settings → General → Zoom link). The link goes into Where
(if it's empty), the event's URL (`o` opens it) and the top of the notes, and
the event is saved. In the event form use `:meet` / `:zoom`; in quick add end
with `@meet` or `@zoom` (`Design review fri 2pm 45m @meet`). Meet uses your
Google account's OAuth sign-in (or the one named in Meet account). It needs the
Google Meet REST API enabled in your Cloud project and a fresh `tern auth
<account>` to grant permission to create meetings.

Meeting invitations show as a card at the top of the message: when, where, who
is coming, your answer, and anything it overlaps in your calendars. `a` / `t` / `n`
answer yes / maybe / no. That sends the organizer a standard reply, puts the
event in your calendar (or takes it out on a no), and marks the message
answered.

`ge` on any message opens a new event with the subject as the title, the date
and time if the message mentions them ("Thursday at 3pm"), any Meet, Zoom or
Teams link in it as the place, and the people on the message and its text in
the notes, ready to adjust and save.

## OAuth (Gmail, Outlook)

```toml
[accounts.oauth]
provider = "google"          # or "microsoft" (tenant = "…" for work accounts)
client_id = "…"
client_secret_cmd = "pass show oauth/google-secret"
```

Run `tern auth <account name>` once. It opens the provider's sign-in page in
your browser and saves the tokens to `~/.local/share/tern/oauth/` (only you can
read them). After that, IMAP, SMTP and the account's CalDAV/CardDAV refresh the
token on their own. You need your own OAuth client: a Google Cloud "desktop
app", or an Azure app registration with `http://localhost` as the redirect URI.
Microsoft 365 calendars use Graph rather than CalDAV and aren't supported yet.

## HTML mail

Mail is shown as plain text. `H` switches the current message to its HTML part,
laid out for the terminal: headings, bold and italics, links, lists, code and
tables keep their shape. Colors from the email are left out so it stays readable
on a dark theme. `o` still opens the real thing in your browser.

## Pictures

Pictures in a message are drawn where they sit in it, in both the text and the
HTML view, using the kitty graphics protocol in kitty and Ghostty, sixel in foot
and Konsole, and iTerm2's in iTerm2 and WezTerm. Other terminals get a rough
half-block version. Pictures the message carries but never places go at the
end. Remote pictures (`<img src="https://…">`) wait for `I`, since loading them
tells the sender you opened the mail. **Pictures** in settings turns it off.

## Settings and mouse

`,` opens settings, and `?` opens it at the Keys tab. The five tabs (`1`–`5` jump between them):

- **General:** threads, folding, undo-send window, mark-read delay, time zone,
  today in the sidebar, notifications and reminder time, Zoom link and Meet
  account, layout widths, icons, background, mouse, pictures, date format, download
  folder and editor. Changes apply
  immediately and are saved to `config.toml`; your comments and layout there
  are kept.
- **Accounts:** add, edit and remove accounts (see [Accounts](#accounts)).
- **Calendars:** calendar and contacts servers, which calendars show, and the
  default for new events.
- **Keys:** every shortcut. `/` filters the list.
- **Theme:** the active Omarchy theme, its palette and a preview.

Mouse:

- Click a folder to open it, or a message to select it.
- Double-click a message (or a folded thread) to read it.
- The wheel scrolls whichever pane it's over. Scrolling the list doesn't
  change which message is selected.
- Click a message in a conversation to open or fold it.
- Drag to select text; letting go copies it to the clipboard (OSC 52, plus
  `wl-copy` or `xclip` for terminals that ignore that).
- In settings, click a tab or a switch. Click a value twice to edit it.
- Turn **Mouse** off in settings for the terminal's own selection.

## Threads

Messages are grouped into conversations from Message-ID, In-Reply-To and
References. When those headers are missing, a message whose subject starts
with "Re:" joins the oldest thread with the same subject. Each conversation is
one row in the list: who took part, how many messages, and the latest date. A
new reply moves the whole thread back to the top. When a folded conversation is
selected, the right pane shows the whole thread, oldest message first. Unread
messages and the latest one are shown in full, the others shrink to one line,
and quoted text is hidden because the earlier messages are already above it.

Archive, delete, junk, move, read/unread and `x` act on the whole conversation.
`s` flags only the latest message. `l` opens a conversation into its messages
so you can act on one: `j` `k` move through them and `h` on any of them folds it
back (`za` does the same). Set `collapse_threads = false` to start with every thread
unfolded, or `threads = false` for a flat list.

Your own replies show up only if they are in the folder you're viewing. tern
doesn't merge in your Sent folder yet.

## How it behaves

- Messages are marked read after they've been open for `mark_read_ms` (800 by
  default), so skimming with `j` doesn't mark everything read.
- Archive, delete and move update the list immediately and the server
  catches up in the background. Each account runs on its own connection
  task, and a dropped connection is reopened and the command retried once.
- New mail is pushed: a second connection waits in IMAP IDLE on the open
  folder, so messages appear as they arrive. Servers without IDLE are checked
  every `poll_secs`. A refresh keeps your cursor on the same message.
- Sent mail is saved to the Sent folder (`save_sent = false` for Gmail).
  Replies set `\Answered` on the original. A send that fails stays in the
  outbox and is retried.
- Contacts are cached in `~/.cache/tern/contacts.json` and refreshed at startup
  when they're over an hour old, after you change a contact, or with `gc`.
  `tern query <text>` prints mutt-style query output, so an editor plugin can
  complete addresses too.
- Passwords come from `password_cmd` (`pass`, `secret-tool`, `op read`, …) or
  `password`.

## Not yet

Microsoft 365 calendars (Graph), editing single occurrences of a recurring
event, PGP, and sending HTML.
