//! The people view: your contacts, searchable, with what you share with each person
//! (recent mail, upcoming meetings) and ways to act on them.

use crate::app::{App, Focus, Mode, Prompt, PromptKind};
use crate::contacts::{Contact, Spec};
use crate::model::Event;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Default)]
pub struct PeopleState {
    pub cursor: usize,
    pub offset: usize,
    pub height: usize,
    pub filter: String,
    /// Asking before deleting the selected contact.
    pub confirm: bool,
}

/// What a contact form is editing (None: a new contact).
#[derive(Clone, Debug)]
pub struct ContactForm {
    pub existing: Option<Contact>,
}

pub const LABELS: [&str; 5] = ["Name", "Email", "More email", "Phone", "Company"];
pub const PLACEHOLDERS: [&str; 5] = ["full name", "main address", "other addresses, comma separated", "phone numbers, comma separated", ""];

fn split(s: &str) -> Vec<String> {
    s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()
}

impl App {
    /// Contacts matching the filter, in list order.
    pub fn people(&self) -> Vec<&Contact> {
        let q = self.people.filter.to_lowercase();
        self.contacts
            .iter()
            .filter(|c| {
                q.is_empty()
                    || c.name.to_lowercase().contains(&q)
                    || c.org.to_lowercase().contains(&q)
                    || c.emails.iter().any(|e| e.to_lowercase().contains(&q))
                    || c.phones.iter().any(|p| p.contains(&q))
            })
            .collect()
    }

    pub fn selected_person(&self) -> Option<Contact> {
        self.people().get(self.people.cursor).map(|c| (*c).clone())
    }

    pub fn people_key(&mut self, k: KeyEvent) -> bool {
        let n = self.people().len();
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if self.people.confirm {
            self.people.confirm = false;
            if matches!(k.code, KeyCode::Char('y') | KeyCode::Char('Y')) {
                if let Some(c) = self.selected_person() {
                    self.write_contact(Some(c), None);
                }
            } else {
                self.say("kept");
            }
            return true;
        }
        let half = (self.people.height / 2).max(1);
        match k.code {
            KeyCode::Char('P') => self.toggle_people(),
            KeyCode::Char('C') => {
                self.screen = crate::calview::View::Mail;
                self.toggle_view();
            }
            KeyCode::Char('j') | KeyCode::Down => self.people.cursor = (self.people.cursor + 1).min(n.saturating_sub(1)),
            KeyCode::Char('k') | KeyCode::Up => self.people.cursor = self.people.cursor.saturating_sub(1),
            KeyCode::Char('d') if ctrl => self.people.cursor = (self.people.cursor + half).min(n.saturating_sub(1)),
            KeyCode::Char('u') if ctrl => self.people.cursor = self.people.cursor.saturating_sub(half),
            KeyCode::Char('g') => self.people.cursor = 0,
            KeyCode::Char('G') => self.people.cursor = n.saturating_sub(1),
            KeyCode::Char('/') => {
                self.mode = Mode::Prompt(Prompt { kind: PromptKind::PeopleFilter, input: self.people.filter.clone(), completions: vec![], sel: None })
            }
            KeyCode::Esc if !self.people.filter.is_empty() => {
                self.people.filter.clear();
                self.people.cursor = 0;
            }
            KeyCode::Char('m') | KeyCode::Enter => match self.selected_person() {
                Some(c) if !c.emails.is_empty() => {
                    let to = c.mailbox(0);
                    self.write_to(&to);
                }
                Some(_) => self.say("no email address for them"),
                None => {}
            },
            KeyCode::Char('a') => self.open_contact_form(None),
            KeyCode::Char('e') => {
                if let Some(c) = self.selected_person() {
                    self.open_contact_form(Some(c));
                }
            }
            KeyCode::Char('d') => {
                if self.selected_person().is_some() {
                    self.people.confirm = true;
                }
            }
            KeyCode::Char('y') => {
                if let Some(e) = self.selected_person().and_then(|c| c.emails.first().cloned()) {
                    self.copy_text(&e);
                }
            }
            KeyCode::Char('$') => {
                self.say("syncing contacts…");
                self.refresh_contacts();
            }
            KeyCode::Tab | KeyCode::Char('l') => self.focus = Focus::Body,
            KeyCode::BackTab | KeyCode::Char('h') => self.focus = Focus::List,
            _ => return false,
        }
        // Keep the cursor in view.
        let h = self.people.height.max(1);
        if self.people.cursor < self.people.offset {
            self.people.offset = self.people.cursor;
        } else if self.people.cursor >= self.people.offset + h {
            self.people.offset = self.people.cursor + 1 - h;
        }
        true
    }

    pub fn toggle_people(&mut self) {
        use crate::calview::View;
        self.screen = if self.screen == View::People { View::Mail } else { View::People };
        if self.screen == View::People {
            self.focus = Focus::List;
            // Meetings with each person come from the calendar.
            if self.cal.rows.is_empty() && !self.cal.calendars.is_empty() {
                self.cal.today_at_top();
            }
        }
    }

    fn open_contact_form(&mut self, existing: Option<Contact>) {
        if self.cfg.dav_sources().is_empty() && !self.demo {
            return self.say_error("add a contacts server first (Settings → Calendars)");
        }
        let (title, values, note) = match &existing {
            Some(c) => (
                format!("Edit {}", c.name),
                vec![
                    c.name.clone(),
                    c.emails.first().cloned().unwrap_or_default(),
                    c.emails.iter().skip(1).cloned().collect::<Vec<_>>().join(", "),
                    c.phones.join(", "),
                    c.org.clone(),
                ],
                c.note.clone(),
            ),
            None => ("New contact".to_string(), vec![String::new(); 5], String::new()),
        };
        let c = crate::composer::Composer::new_form(&title, &LABELS, &PLACEHOLDERS, &values, &note, "Notes go in the body.");
        let mut c = c;
        c.contact = Some(ContactForm { existing });
        self.mode = Mode::Compose(Box::new(c));
    }

    pub fn save_contact(&mut self, c: Box<crate::composer::Composer>) {
        let Some(form) = c.contact.clone() else { return };
        let v = c.values();
        let mut emails = split(&v[1]);
        emails.extend(split(&v[2]));
        let spec = Spec { name: v[0].clone(), emails, phones: split(&v[3]), org: v[4].clone(), note: c.body_text() };
        if spec.name.is_empty() && spec.emails.is_empty() {
            self.say_error("a contact needs a name or an email");
            self.mode = Mode::Compose(c);
            return;
        }
        if let Some(bad) = spec.emails.iter().find(|e| !e.contains('@')) {
            self.say_error(format!("`{bad}` doesn't look like an email address"));
            self.mode = Mode::Compose(c);
            return;
        }
        self.write_contact(form.existing, Some(spec));
    }

    /// Create, update (spec Some) or delete (spec None) on the server, then resync.
    fn write_contact(&mut self, existing: Option<Contact>, spec: Option<Spec>) {
        if self.demo {
            match (&existing, &spec) {
                (Some(old), Some(s)) => {
                    if let Some(c) = self.contacts.iter_mut().find(|c| c.name == old.name) {
                        c.name = s.name.clone();
                        c.emails = s.emails.clone();
                        c.phones = s.phones.clone();
                        c.org = s.org.clone();
                        c.note = s.note.clone();
                    }
                }
                (Some(old), None) => self.contacts.retain(|c| c.name != old.name),
                (None, Some(s)) => self.contacts.push(Contact {
                    name: s.name.clone(),
                    emails: s.emails.clone(),
                    phones: s.phones.clone(),
                    org: s.org.clone(),
                    note: s.note.clone(),
                    ..Default::default()
                }),
                _ => {}
            }
            self.contacts.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            return self.say("demo: changed here only");
        }
        self.say("saving contact…");
        let (cfg, tx) = (self.cfg.clone(), self.events_tx.clone());
        tokio::spawn(async move {
            let ev = match crate::dav::write_contact(cfg.clone(), existing, spec).await {
                Ok(msg) => match crate::dav::refresh_contacts(cfg).await {
                    Ok((c, _)) => {
                        let _ = tx.send(Event::Contacts(c));
                        Event::Note(msg)
                    }
                    Err(e) => Event::Error(usize::MAX, format!("contacts: {e:#}")),
                },
                Err(e) => Event::Error(usize::MAX, format!("contact: {e:#}")),
            };
            let _ = tx.send(ev);
        });
    }
}
