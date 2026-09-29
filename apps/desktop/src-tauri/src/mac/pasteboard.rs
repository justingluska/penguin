//! Files on the general pasteboard, the way Finder's Copy puts them there:
//! one `public.file-url` item per file (NSURL is NSPasteboardWriting). A
//! paste in Finder, Mail, Slack or a web page gets the file; WebKit hands
//! pasted file URLs to the page as `clipboardData.files`, so Penguin's own
//! composer attaches it too.

use std::path::Path;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{NSPasteboard, NSPasteboardWriting};
use objc2_foundation::{NSArray, NSString, NSURL};

/// Replace the general pasteboard's contents with these files. False when
/// the pasteboard refused them. Call on the main thread.
pub fn write_files(paths: &[&Path]) -> bool {
    let items: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = paths
        .iter()
        .map(|p| {
            let url = NSURL::fileURLWithPath(&NSString::from_str(&p.to_string_lossy()));
            ProtocolObject::from_retained(url)
        })
        .collect();
    let items = NSArray::from_retained_slice(&items);
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    pb.writeObjects(&items)
}
