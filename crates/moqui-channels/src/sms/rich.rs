/*
 * This software is in the public domain under CC0 1.0 Universal plus a
 * Grant of Patent License.
 *
 * To the extent possible under law, the author(s) have dedicated all
 * copyright and related and neighboring rights to this software to the
 * public domain worldwide. This software is distributed without any
 * warranty.
 *
 * You should have received a copy of the CC0 Public Domain Dedication
 * along with this software (see the LICENSE.md file). If not, see
 * <http://creativecommons.org/publicdomain/zero/1.0/>.
 */

//! A rich message's cards and suggestions as a provider sees them (plans/Comms-Messaging.md §18.3 phase 13): the parse of
//! [`moqui_core::service::chat_bridge::DeliveryRequest::rich`], the event's `content.rich` that `ChatRich.js` checked
//! and cleaned. An RCS provider sends it in its own form (Twilio's Content, Sinch's card, carousel and choice messages);
//! every other provider sends the message's fallback (its text) and files, as for any text.
//!
//! The parse is defensive: a part it cannot read is left out, and a rich message with nothing left is none, so its
//! fallback goes.

use serde_json::Value as Json;

/// What a rich message is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RichKind {
    /// Words with suggestion chips.
    Text,
    /// One card.
    Card,
    /// Cards side by side.
    Carousel,
}

/// A suggestion the person can tap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Suggestion {
    /// A reply: its words come back to us, with its postback.
    Reply { text: String, postback: String },
    /// A link the phone opens.
    Url { text: String, url: String },
    /// A number the phone calls.
    Dial { text: String, phone: String },
}

impl Suggestion {
    fn of(value: &Json) -> Option<Self> {
        let text = words(value, "text")?;
        match value.get("type").and_then(Json::as_str)? {
            "reply" => Some(Suggestion::Reply {
                postback: words(value, "postback").unwrap_or_else(|| text.clone()),
                text,
            }),
            "url" => {
                let url = words(value, "url")?;
                (url.starts_with("https://") || url.starts_with("http://")).then_some(Suggestion::Url { text, url })
            }
            "dial" => Some(Suggestion::Dial {
                phone: words(value, "phone")?,
                text,
            }),
            _ => None,
        }
    }

    /// Its words on the chip or button.
    pub fn text(&self) -> &str {
        match self {
            Suggestion::Reply { text, .. } | Suggestion::Url { text, .. } | Suggestion::Dial { text, .. } => text,
        }
    }
}

/// A card.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Card {
    pub title: Option<String>,
    pub description: Option<String>,
    /// Its image: the index of the message's file in [`crate::sms::Outgoing::media_urls`].
    pub image: Option<usize>,
    pub suggestions: Vec<Suggestion>,
}

/// A Twilio Content an app made itself (`providerContent.twilio`): sent as it is, never made or deleted here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TwilioContent {
    pub content_sid: String,
    /// `ContentVariables`, a JSON object as text.
    pub variables: Option<String>,
}

/// A rich message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rich {
    pub kind: RichKind,
    /// A text's words.
    pub text: Option<String>,
    pub cards: Vec<Card>,
    /// A text's chips.
    pub suggestions: Vec<Suggestion>,
    pub twilio: Option<TwilioContent>,
}

fn words(value: &Json, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Json::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

fn suggestions(value: &Json) -> Vec<Suggestion> {
    value
        .get("suggestions")
        .and_then(Json::as_array)
        .map(|list| list.iter().filter_map(Suggestion::of).collect())
        .unwrap_or_default()
}

impl Rich {
    /// The rich message of `content.rich`, with `media_count` files on the message (a card's `attachmentSeq` is 1 to it);
    /// none when it is not one.
    pub fn of(value: &Json, media_count: usize) -> Option<Self> {
        let kind = match value.get("kind").and_then(Json::as_str)? {
            "text" => RichKind::Text,
            "card" => RichKind::Card,
            "carousel" => RichKind::Carousel,
            _ => return None,
        };
        let cards: Vec<Card> = value
            .get("cards")
            .and_then(Json::as_array)
            .map(|list| {
                list.iter()
                    .map(|card| Card {
                        title: words(card, "title"),
                        description: words(card, "description"),
                        image: card
                            .get("attachmentSeq")
                            .and_then(Json::as_u64)
                            .and_then(|seq| usize::try_from(seq).ok())
                            .filter(|seq| (1..=media_count).contains(seq))
                            .map(|seq| seq - 1),
                        suggestions: suggestions(card),
                    })
                    .filter(|card| card.title.is_some() || card.description.is_some())
                    .collect()
            })
            .unwrap_or_default();
        let twilio = value.pointer("/providerContent/twilio").and_then(|twilio| {
            let sid = words(twilio, "contentSid").filter(|sid| sid.starts_with("HX"))?;
            let variables = match twilio.get("contentVariables") {
                Some(Json::String(text)) => Some(text.clone()),
                Some(object @ Json::Object(_)) => Some(object.to_string()),
                _ => None,
            };
            Some(TwilioContent {
                content_sid: sid,
                variables,
            })
        });
        let rich = Rich {
            kind,
            text: words(value, "text"),
            suggestions: suggestions(value),
            cards,
            twilio,
        };
        let usable = match kind {
            RichKind::Text => rich.text.is_some() && !rich.suggestions.is_empty(),
            RichKind::Card => rich.cards.len() == 1,
            RichKind::Carousel => rich.cards.len() >= 2,
        };
        (usable || rich.twilio.is_some()).then_some(rich)
    }

    /// The first image of its cards (a fallback MMS's picture).
    pub fn first_image(&self) -> Option<usize> {
        self.cards.iter().find_map(|card| card.image)
    }
}
