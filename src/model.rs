//! Data shared between the UI and the mail backends.

use chrono::{DateTime, Local};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Inbox,
    Sent,
    Drafts,
    Archive,
    Trash,
    Junk,
    All,
    Flagged,
    Other,
}

#[derive(Debug, Clone)]
pub struct Folder {
    /// Raw IMAP name, used for commands.
    pub name: String,
    /// Human name (last path segment, modified-UTF-7 decoded).
    pub label: String,
    pub depth: usize,
    pub role: Role,
    pub unread: u32,
    pub selectable: bool,
}

#[derive(Debug, Clone)]
pub struct Envelope {
    pub uid: u32,
    pub from_name: String,
    pub from_addr: String,
    pub to: String,
    pub subject: String,
    pub date: Option<DateTime<Local>>,
    pub seen: bool,
    pub flagged: bool,
    pub answered: bool,
    pub has_attachment: bool,
    pub message_id: String,
    pub in_reply_to: String,
    pub references: Vec<String>,
}

impl Envelope {
    /// First recipient's display name, or address.
    pub fn recipient(&self) -> &str {
        let first = self.to.split(", ").next().unwrap_or("");
        match first.find(" <") {
            Some(i) => first[..i].trim_matches('"'),
            None => first,
        }
    }

    pub fn sender(&self) -> &str {
        if self.from_name.is_empty() {
            &self.from_addr
        } else {
            &self.from_name
        }
    }
}

#[derive(Debug, Clone)]
pub struct Attachment {
    pub name: String,
    pub size: usize,
}

#[derive(Debug, Clone)]
pub struct Body {
    pub uid: u32,
    pub from: String,
    pub reply_to: String,
    pub to: String,
    pub cc: String,
    pub subject: String,
    pub date: String,
    pub message_id: String,
    pub references: String,
    pub text: String,
    pub html: Option<String>,
    pub attachments: Vec<Attachment>,
    pub list_unsubscribe: Option<String>,
    /// The full RFC 822 message, kept for saving attachments and forwarding.
    pub raw: Vec<u8>,
}

/// Requests from the UI to an account worker.
#[derive(Debug)]
pub enum Cmd {
    Folders,
    Select { folder: String },
    Body { folder: String, uid: u32 },
    /// Several bodies in one round trip, for the conversation view.
    Bodies { folder: String, uids: Vec<u32> },
    SetFlag { folder: String, uids: Vec<u32>, flag: &'static str, on: bool },
    Move { folder: String, uids: Vec<u32>, to: String },
    Expunge { folder: String, uids: Vec<u32> },
    Append { folder: String, raw: Vec<u8>, flags: &'static str },
    Search { folder: String, query: String },
    Poll { folder: String },
}

/// Responses from a worker back to the UI. Every event carries its account index.
#[derive(Debug)]
pub enum Event {
    Status(usize, String),
    Error(usize, String),
    Connected(usize),
    Folders(usize, Vec<Folder>),
    Envelopes { acct: usize, folder: String, items: Vec<Envelope>, total: u32 },
    Body { acct: usize, folder: String, body: Box<Body> },
    Removed { acct: usize, folder: String, uids: Vec<u32> },
    /// The server pushed a change (IDLE) to a folder; time to refresh it.
    Changed { acct: usize, folder: String },
    SearchResult { acct: usize, folder: String, items: Vec<Envelope>, query: String },
    Contacts(Vec<crate::contacts::Contact>),
    Sent(String),
    /// Calendars after a sync, and any sources that failed.
    Calendars(Vec<crate::dav::CachedCalendar>, Vec<String>),
    SendFailed(Box<crate::compose::Draft>, String),
}
