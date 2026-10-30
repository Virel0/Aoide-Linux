//! The back stack: sidebar sections, pages opened from other pages, Back,
//! Retry, and the messages that belong to one page.

use iced::Task;

use super::{App, Entry, Message, Origin};
use crate::nav::BackStack;
use crate::screens::{self, Ctx, Page, PageMsg, Route, Section};

impl App {
    /// A sidebar click: the section's root page, starting the stack over.
    pub(super) fn show_section(&mut self, section: Section) -> Task<Message> {
        let store = self.store.clone();
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        signed.section = section.clone();
        signed.menu = None;
        let key = signed.next_key;
        signed.next_key += 1;
        let ctx = Ctx {
            client: &signed.connection.client,
            store: &store,
            key,
        };
        let (page, task) = Page::section(&section, &ctx);
        let entry = Entry {
            key,
            page,
            origin: Origin::Section(section),
        };
        match signed.nav.as_mut() {
            Some(nav) => nav.reset(entry),
            None => signed.nav = Some(BackStack::new(entry)),
        }
        task
    }

    /// Settings, at one category: the command palette's way in.
    pub(super) fn open_settings(&mut self, category: screens::settings::Category) -> Task<Message> {
        let task = self.show_section(Section::Settings);
        if let Some(Page::Settings(state)) = self
            .session
            .as_mut()
            .and_then(|s| s.nav.as_mut())
            .and_then(|nav| nav.iter_mut().last())
            .map(|e| &mut e.page)
        {
            state.category = category;
        }
        task
    }

    /// Opens a page on top of the current one.
    pub(super) fn open(&mut self, route: Route) -> Task<Message> {
        let store = self.store.clone();
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        signed.menu = None;
        let key = signed.next_key;
        signed.next_key += 1;
        let ctx = Ctx {
            client: &signed.connection.client,
            store: &store,
            key,
        };
        let (page, task) = Page::route(&route, &ctx);
        let entry = Entry {
            key,
            page,
            origin: Origin::Route(route),
        };
        match signed.nav.as_mut() {
            Some(nav) => nav.push(entry),
            None => signed.nav = Some(BackStack::new(entry)),
        }
        task
    }

    pub(super) fn back(&mut self) -> Task<Message> {
        if let Some(nav) = self.session.as_mut().and_then(|s| s.nav.as_mut()) {
            nav.back();
        }
        Task::none()
    }

    /// Retry: the page opens again where it is in the stack, under a new key
    /// so an answer still on its way to the old one goes nowhere, and runs
    /// its requests again.
    pub(super) fn retry(&mut self, key: u64) -> Task<Message> {
        let store = self.store.clone();
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        let new_key = signed.next_key;
        let Some(entry) = signed
            .nav
            .as_mut()
            .and_then(|nav| nav.iter_mut().find(|e| e.key == key))
        else {
            return Task::none();
        };
        signed.next_key += 1;
        signed.menu = None;
        let ctx = Ctx {
            client: &signed.connection.client,
            store: &store,
            key: new_key,
        };
        let (page, task) = match &entry.origin {
            Origin::Section(section) => Page::section(section, &ctx),
            Origin::Route(route) => Page::route(route, &ctx),
        };
        entry.key = new_key;
        entry.page = page;
        self.failure_details = false;
        task
    }

    pub(super) fn page_message(&mut self, key: u64, message: PageMsg) -> Task<Message> {
        if let PageMsg::Settings(message) = message {
            return self.settings_message(key, message);
        }
        if let PageMsg::Equalizer(message) = message {
            return self.equalizer_message(key, message);
        }
        let store = self.store.clone();
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        let Some(nav) = signed.nav.as_mut() else {
            return Task::none();
        };
        let Some(entry) = nav.iter_mut().find(|e| e.key == key) else {
            return Task::none();
        };
        let ctx = Ctx {
            client: &signed.connection.client,
            store: &store,
            key,
        };
        entry.page.update(message, &ctx)
    }
}
