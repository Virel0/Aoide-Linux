//! Settings → Smart Search as drawn: the key (write-only) and the model.

use aoide_integrations::openrouter::ApiKey;
use iced::widget::{Column, button, column, row, text, text_input};
use iced::{Center, Element, Fill};

use super::{Draft, KEY_FIELD, LISTED, Message, SmartSettings, State, matching, price_label};
use crate::app::Message as AppMessage;
use crate::screens::Load;
use crate::{theme, widgets};

/// Settings → Smart Search: the key (write-only) and the model.
pub fn settings_section<'a>(
    state: &'a State,
    settings: &'a SmartSettings,
    details: bool,
) -> Element<'a, AppMessage> {
    let rows = key_rows(state);
    let rows = model_rows(rows, state, settings, details);
    crate::screens::settings::section(
        "",
        rows,
        "Search by description and Mixes ask a language model on OpenRouter to turn your words \
         into filters; your server then finds the songs. Only the words you typed and your \
         library's genre names are sent, with the key, to OpenRouter, and only when you ask. \
         The key stays in your system keyring. Each request is billed to your OpenRouter account \
         at the model's price; small models cost a fraction of a cent.",
    )
}

/// The key: saved, cleared, or said to be set. Never shown.
fn key_rows(state: &State) -> Column<'_, AppMessage> {
    let send = |m: Message| AppMessage::Smart(m);
    let mut rows: Column<'_, AppMessage> = column![];
    let key_line = match state.key_set {
        None => "Checking the keyring…",
        Some(true) => "A key is set.",
        Some(false) => "No key is set.",
    };
    let mut key_row = row![
        column![
            text("OpenRouter key"),
            widgets::secondary(key_line).size(13)
        ]
        .spacing(2)
        .width(Fill)
    ]
    .spacing(8)
    .align_y(Center);
    if state.has_key() {
        key_row = key_row.push(
            button(text("Clear").size(13))
                .style(theme::destructive)
                .padding([4, 10])
                .on_press_maybe((!state.saving).then(|| send(Message::ClearKey))),
        );
    }
    rows = rows.push(key_row);
    let can_save = !state.saving && ApiKey::new(&state.draft.0).is_some();
    rows = rows.push(
        row![
            text_input(
                if state.has_key() {
                    "Paste a new key to replace it"
                } else {
                    "Paste your OpenRouter key"
                },
                &state.draft.0,
            )
            .id(KEY_FIELD)
            .secure(true)
            .on_input(|s| AppMessage::Smart(Message::KeyTyped(Draft(s))))
            .on_submit_maybe(can_save.then(|| send(Message::SaveKey)))
            .padding(8)
            .style(theme::field),
            button(text("Save").size(13))
                .style(theme::tonal)
                .padding([6, 12])
                .on_press_maybe(can_save.then(|| send(Message::SaveKey))),
        ]
        .spacing(8)
        .align_y(Center),
    );
    if let Some(error) = &state.key_error {
        rows = rows.push(text(error.as_str()).size(13));
    }
    rows
}

/// The model: picked from OpenRouter's list, or any id typed.
fn model_rows<'a>(
    mut rows: Column<'a, AppMessage>,
    state: &'a State,
    settings: &'a SmartSettings,
    details: bool,
) -> Column<'a, AppMessage> {
    let send = |m: Message| AppMessage::Smart(m);
    rows = rows.push(
        row![
            column![text("Model"), widgets::secondary(settings.model()).size(13)]
                .spacing(2)
                .width(Fill),
            button(text("Show models").size(13))
                .style(theme::tonal)
                .padding([4, 10])
                .on_press_maybe(
                    (!matches!(state.models, Some(Load::Loading)))
                        .then(|| send(Message::ShowModels))
                ),
        ]
        .spacing(8)
        .align_y(Center),
    );
    let typed = state.model_filter.trim();
    rows = rows.push(
        row![
            text_input("Search the models, or type a model id", &state.model_filter)
                .on_input(|s| AppMessage::Smart(Message::ModelTyped(s)))
                .on_submit_maybe(
                    (!typed.is_empty()).then(|| send(Message::UseModel(typed.to_owned())))
                )
                .padding(8)
                .style(theme::field),
            button(text("Use this ID").size(13))
                .style(theme::tonal)
                .padding([6, 12])
                .on_press_maybe(
                    (!typed.is_empty()).then(|| send(Message::UseModel(typed.to_owned())))
                ),
        ]
        .spacing(8)
        .align_y(Center),
    );
    match &state.models {
        None => rows,
        Some(Load::Loading) => rows.push(widgets::secondary("Loading OpenRouter's models…")),
        Some(Load::Failed(error)) => rows.push(widgets::failure(
            error,
            Some(send(Message::ShowModels)),
            details,
        )),
        Some(Load::Ready(models)) => model_list(rows, models, typed, settings),
    }
}

/// The models matching what is typed, the one in use lit.
fn model_list<'a>(
    mut rows: Column<'a, AppMessage>,
    models: &'a [aoide_integrations::openrouter::Model],
    typed: &str,
    settings: &SmartSettings,
) -> Column<'a, AppMessage> {
    let found = matching(models, typed, LISTED);
    if found.is_empty() {
        rows = rows.push(widgets::secondary("No model matches that.").size(13));
    }
    for model in found {
        let chosen = model.id == settings.model();
        rows = rows.push(
            button(
                row![
                    column![
                        text(model.name.as_str()).size(14),
                        widgets::secondary(model.id.as_str()).size(12),
                    ]
                    .spacing(1)
                    .width(Fill),
                    widgets::secondary(price_label(model)).size(12),
                ]
                .spacing(10)
                .align_y(Center),
            )
            .width(Fill)
            .padding([4, 8])
            .style(if chosen {
                theme::row_selected
            } else {
                theme::row
            })
            .on_press(AppMessage::Smart(Message::UseModel(model.id.clone()))),
        );
    }
    rows
}
