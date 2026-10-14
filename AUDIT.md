# Audit: the ten things a unified mail · calendar · contacts tool needed most

Written 2026-10-14. tern had good reading, writing and calendar tools, but it
could lose data, it treated your business and personal accounts as separate
worlds, and mail, calendar and contacts only half knew about each other.
These ten features are ranked by how much they matter for everyday business
and personal use.

## Never lose anything

1. **Outbox with undo send.** A sent message waits a few seconds (`u` takes it
   back into the composer), then goes out from a queue saved on disk. A failed
   send (offline, server down) stays queued and retries; nothing is dropped
   because the network blinked or tern quit.
2. **Drafts that survive.** The composer saves itself to disk as you type and
   offers to pick up where you left off after a crash or quit. Messages in a
   Drafts folder open back into the composer (`⏎`), and the server copy is
   removed once the message is sent.

## One place for both lives

3. **Unified inbox.** "All inboxes" sits at the top of the sidebar: every
   account's INBOX in one list, each row tagged with its account, with triage
   and replies working as usual.
4. **Send from the right identity.** The composer's From field cycles through
   your accounts (`^f`). Replies start from the account the mail arrived on,
   with that account's signature.

## Mail, calendar and contacts as one system

5. **Contacts screen.** `P` opens people: search, details, recent mail with
   that person, and upcoming meetings with them. Write to them, add new
   contacts, edit or delete them; everything syncs to CardDAV.
6. **Notifications and reminders.** A desktop notification when new mail
   arrives and before each meeting (the event's own alarm, or 10 minutes).
7. **Today in the mail view.** The mail sidebar ends with your next few events,
   so you see your day while triaging.
8. **Email → event.** `ge` on a message opens a new event prefilled from it:
   the subject as the title, a date and time if the mail mentions one, the
   people on the thread, and a pointer back to the message.
9. **Snooze.** `b` hides a message until a time you type (`tomorrow 9am`,
   `mon`, `3h`), then puts it back in the inbox, unread, with a notification.

## Confidence

10. **`tern doctor`.** One command that checks the config, every password
    command, IMAP and SMTP logins, OAuth tokens, calendar and contacts
    discovery, the theme and the editor, and says exactly what's wrong.
