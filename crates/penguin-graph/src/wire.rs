//! Graph JSON shapes Penguin reads, and the `$select` lists it asks for.

use serde::Deserialize;

/// Enough to file, label and list a message (delta pages, search, listings).
pub(crate) const SELECT_LIGHT: &str = "id,conversationId,receivedDateTime,subject,bodyPreview,from,sender,toRecipients,ccRecipients,isRead,isDraft,flag,categories,parentFolderId,internetMessageId,hasAttachments";
/// Everything a stored message needs.
pub(crate) const SELECT_FULL: &str = "id,conversationId,receivedDateTime,sentDateTime,subject,bodyPreview,body,from,sender,toRecipients,ccRecipients,bccRecipients,replyTo,isRead,isDraft,flag,categories,parentFolderId,internetMessageId,hasAttachments,internetMessageHeaders";
/// Attachment metadata only (no `contentBytes`).
pub(crate) const EXPAND_ATTACHMENTS: &str =
    "attachments($select=id,name,contentType,size,isInline)";
/// PR_MESSAGE_SIZE, for "Message details".
pub(crate) const SIZE_PROPERTY: &str = "Integer 0x0E08";

#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct EmailAddress {
    pub name: Option<String>,
    pub address: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Recipient {
    pub email_address: EmailAddress,
}

#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct ItemBody {
    pub content_type: String,
    pub content: String,
}

#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Flag {
    pub flag_status: String,
}

#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
#[serde(default)]
pub(crate) struct Header {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Attachment {
    #[serde(rename = "@odata.type")]
    pub odata_type: String,
    pub id: String,
    pub name: Option<String>,
    pub content_type: Option<String>,
    pub size: u64,
    pub is_inline: bool,
    pub content_id: Option<String>,
}

impl Attachment {
    pub fn is_file(&self) -> bool {
        self.odata_type.is_empty() || self.odata_type.ends_with("fileAttachment")
    }
    pub fn is_item(&self) -> bool {
        self.odata_type.ends_with("itemAttachment")
    }
}

#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
#[serde(default)]
pub(crate) struct ExtendedProperty {
    pub id: String,
    pub value: String,
}

/// A message (or, in a delta page, a change). Absent fields are None so a
/// partial change never reads as "false".
#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct WireMessage {
    pub id: String,
    /// Present on delta entries for messages that left the folder.
    #[serde(rename = "@removed")]
    pub removed: Option<serde_json::Value>,
    pub conversation_id: Option<String>,
    pub received_date_time: Option<String>,
    pub sent_date_time: Option<String>,
    pub subject: Option<String>,
    pub body_preview: Option<String>,
    pub body: Option<ItemBody>,
    pub from: Option<Recipient>,
    pub sender: Option<Recipient>,
    pub to_recipients: Option<Vec<Recipient>>,
    pub cc_recipients: Option<Vec<Recipient>>,
    pub bcc_recipients: Option<Vec<Recipient>>,
    pub reply_to: Option<Vec<Recipient>>,
    pub is_read: Option<bool>,
    pub is_draft: Option<bool>,
    pub flag: Option<Flag>,
    pub categories: Option<Vec<String>>,
    pub parent_folder_id: Option<String>,
    pub internet_message_id: Option<String>,
    pub has_attachments: Option<bool>,
    pub internet_message_headers: Option<Vec<Header>>,
    pub attachments: Option<Vec<Attachment>>,
    pub single_value_extended_properties: Option<Vec<ExtendedProperty>>,
}

/// A collection page (`value` + `@odata.nextLink` / `@odata.deltaLink`).
#[derive(Debug, Deserialize)]
pub(crate) struct Page<T> {
    #[serde(default = "Vec::new")]
    pub value: Vec<T>,
    #[serde(rename = "@odata.nextLink", default)]
    pub next_link: Option<String>,
    #[serde(rename = "@odata.deltaLink", default)]
    pub delta_link: Option<String>,
    #[serde(rename = "@odata.count", default)]
    pub count: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct MailFolder {
    pub id: String,
    pub display_name: String,
    pub parent_folder_id: Option<String>,
    pub child_folder_count: u32,
    pub total_item_count: u64,
    pub unread_item_count: u64,
}

#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Category {
    pub display_name: String,
}

/// An id-and-conversation answer (created drafts, moves).
#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Created {
    pub id: String,
    pub conversation_id: Option<String>,
    pub parent_folder_id: Option<String>,
    pub is_draft: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct UploadSession {
    pub upload_url: String,
}
