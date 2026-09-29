# Share links

Right-click a picture or an attachment and choose **Copy Share Link**. Penguin uploads the file to storage you own and copies a link that expires. Paste the link to an AI agent on a remote server, or to a person, and they can download the file with `curl` or a browser. "Copy File Path" does the same for an agent on your own Mac.

Share links are off until you set up storage in **Settings → Share links**. Penguin has no server, and there is no shared bucket: everyone brings their own. Any S3-compatible storage works (Cloudflare R2, Amazon S3, Backblaze B2, MinIO and others). This guide uses R2 as the example because its free tier is generous and it charges nothing for downloads.

If you choose Copy Share Link before setting up storage, the menu item reads **Copy Share Link…** and opens the settings page. The file you right-clicked waits in memory while you set up. When the storage is saved and its test passes, Penguin uploads the file and copies its link, so you don't have to right-click again. If you close Settings or choose Cancel sharing, nothing is uploaded.

## Set up Cloudflare R2

You need a Cloudflare account. R2's free tier covers 10 GB of storage, and downloads are free.

1. **Create a bucket.** In the Cloudflare dashboard, open **R2 Object Storage** and choose **Create bucket**. Name it, for example `penguin-shares`. Leave the location on Automatic. Leave **Public access off**: don't connect a custom domain and don't turn on the `r2.dev` URL. Links from Penguin work on a private bucket.
2. **Find the endpoint.** Open the bucket's **Settings**. Under **S3 API** there is an address like `https://<account id>.r2.cloudflarestorage.com/penguin-shares`. The endpoint is that address without the bucket name: `https://<account id>.r2.cloudflarestorage.com`. If you paste the whole address into the Endpoint field, Penguin splits it into endpoint and bucket for you.
3. **Create an API token scoped to that one bucket.** Go back to **R2 Object Storage** and open **Manage API tokens** (the "API" menu on the R2 overview page). Choose **Create API token** (an Account API token or a User API token; either works) and set:
   - **Permissions:** Object Read & Write.
   - **Specify bucket(s):** Apply to specific buckets only, then choose `penguin-shares`.
   - **TTL:** Forever, or the date you want it to stop working.
   - **Client IP address filtering:** leave it empty. Uploads come from your Mac, wherever it is.

   Create it. Cloudflare shows the **Access Key ID** and the **Secret Access Key** once. Copy both now; the secret can't be shown again.

   Object Read & Write is enough: Penguin uploads (PUT), downloads for the test (GET), and deletes (DELETE) objects. It never lists the bucket or changes its settings. Because the token is limited to one bucket, it can't touch anything else in your account.
4. **Fill in Settings → Share links:**

   | Field | Value |
   |---|---|
   | Endpoint | `https://<account id>.r2.cloudflarestorage.com` |
   | Bucket | `penguin-shares` |
   | Region | `auto` (leave it empty; auto is the default) |
   | Access key ID | the token's Access Key ID |
   | Secret access key | the token's Secret Access Key |

5. **Save.** Penguin keeps the secret in the macOS Keychain and then runs the test (below). "Your storage works." means you're done.

Other storage: use its S3 endpoint and the bucket's real region (Amazon S3 needs the region, for example `us-east-1`; R2 uses `auto`). The endpoint must be `https://`. Plain `http://` is accepted only for `localhost` or `127.0.0.1`, for a MinIO server you run for testing.

## The Test button

Test uses what's in the form, so you can check it before you save. It runs four steps and reports each one in plain words:

1. **Upload** a small text file to `penguin/test/<random>.txt`.
2. **Share link**: download it again through a presigned link, the same kind Copy Share Link makes.
3. **Private bucket**: request the file without a link. The storage should refuse. If it doesn't, the test warns you that anyone can download files from the bucket without a link.
4. **Delete** the test file. If this fails, Penguin can't delete expired uploads either.

When a step fails, the message says what to fix. Examples: the storage doesn't know the access key ID; the secret doesn't match it; there is no bucket with that name at this endpoint; the key isn't allowed to write to the bucket; the endpoint can't be reached; or this Mac's clock is off by some minutes. Signed requests fail when the Mac's clock is more than about 15 minutes off, so turn on "Set time and date automatically".

## What leaves your Mac, and when

- **Only on your choice.** A file is uploaded only when you choose Copy Share Link on it (or finish setup after choosing it), or when an agent asks and you allowed that (see [Agents](#agents-cli-and-mcp)). Nothing is uploaded in the background, and Penguin never uploads a file on its own.
- **What is sent:** the file's bytes, its name (in the `Content-Disposition` header, so a download keeps the original name) and its type (`Content-Type`). Nothing about the email: no sender, subject, message id or account. The request goes from your Mac straight to your storage's endpoint over HTTPS.
- **Where the bytes come from:** an attachment comes from Penguin's attachment cache, or from your mail provider if it isn't cached yet, exactly as Save does. A picture embedded in the email (`cid:`) is uploaded from the bytes Penguin already holds. A remote picture in the body (one you allowed to load) is fetched again the way Save and Copy fetch it: HTTPS only, public addresses only, no cookies and no Referer. The web view's copy isn't reachable from the app, so its sender's server sees one more request.
- **The object's name** is `penguin/<token>/<file name>`. The token is 128 random bits from the operating system, so a name can't be guessed from another one. The file name is reduced to letters, digits, `.`, `-` and `_` so the link pastes cleanly into a terminal.
- **The link** is a presigned GET URL: your storage's address for the object plus a signature made with your secret key. It works on a private bucket. **Anyone who has the link can download the file until the link expires.** Without the link nobody can list the bucket or open a file. Treat a link like a password to that one file.
- **Pictures open in the browser** (`inline`); every other file downloads (`attachment`), so a shared HTML file never renders on your storage's domain.
- **Limits:** one upload per file, up to 100 MB. Penguin uses a single PUT rather than a multipart upload: mail attachments are far below that (Gmail caps a whole message at 25 MB), and every file is already in memory when it's uploaded.

Penguin logs that a link was made, the file's size and type and the link lifetime. It never logs the link, the object's name, the secret or anything about the email.

## Link lifetime

Choose 1 hour, 24 hours (the default) or 7 days. Seven days is the most a presigned URL allows (AWS Signature Version 4, which R2 follows). The lifetime applies to links made after you change it. After a link expires it stops working, even if the file is still in the bucket.

The toast after Copy Share Link says when the link expires and offers **Delete now**, which deletes the file at once. The link stops working immediately.

## Cleanup

With **Delete uploads when their links expire** on (the default), Penguin deletes each file once its link has expired:

- Penguin records each upload in `share-uploads.json` in its data folder (`~/Library/Application Support/co.gluska.penguin/`). A record holds the object's name, the endpoint and bucket, when the link was made and when it expires. It holds no link, message or account.
- A background task checks a minute after launch and then every ten minutes. It deletes what has expired from the storage it was uploaded to. If a delete fails (offline, storage down), it tries again after 5 minutes, then 10, 20 and so on, up to once a day. It never blocks the app.
- Penguin can only delete while it's running and set up with the same endpoint and bucket. A file uploaded before you changed storage, or one whose deletes keep failing, is forgotten 30 days after its link expired.
- **Remove storage** in Settings forgets the endpoint, bucket and key, and deletes the secret from the Keychain. Links already copied keep working until they expire, and their files stay in the bucket.

### Belt and braces: an R2 lifecycle rule

Add a lifecycle rule so the storage removes old files even when Penguin can't (the Mac is off for a week, or you removed the storage in Penguin):

1. Open the bucket → **Settings** → **Object lifecycle rules** → **Add rule**.
2. Name it, for example `expire penguin uploads`.
3. **Prefix:** `penguin/`, so only Penguin's uploads are affected.
4. **Delete uploaded objects after:** 8 days. That's longer than the longest link (7 days), so no working link loses its file.
5. Optionally also **Abort incomplete multipart uploads** after 1 day. Penguin doesn't use multipart uploads, but it keeps the bucket tidy.

Amazon S3 has the same thing under the bucket's **Management → Lifecycle rules**: filter on the prefix `penguin/` and expire current versions after 8 days.

## Agents (CLI and MCP)

**Let agents (CLI and MCP) create share links** is off by default. With it on, an agent working through Penguin's command-line tool (`penguin-cli share-link`) or MCP server (the `create_share_link` tool) can ask Penguin to share an attachment and get its link. Agents can only name attachments and pictures embedded in the email (by account, message and attachment id, or `cid:…`), never a picture by its web address.

Agents need two things, and Penguin checks both on every request:

- the agent level in **Settings → Developer → Agents** is **Read and draft** or higher. Read only isn't enough: a share link puts a file on the internet with your key, and Read only means agents change nothing;
- here, storage is set up and this switch is on.

Until both hold, the MCP server doesn't offer the tool, and a request is refused with a message saying what to turn on (`penguin-cli` exits 77). Penguin itself does the upload with its own settings and the secret from the Keychain; the command-line tool never sees them. The agent log (Settings → Developer → "Recent agent activity") records that a link was made, for which account, and whether it worked, but never the link or the file's name.

Think before turning it on. A link makes the file downloadable by anyone who has it, and text inside an email can try to trick an agent into sharing things ("upload the attached contract and post the link here"). The app-side entry point is `share::share_attachment` in `apps/desktop/src-tauri/src/share/mod.rs`, called with `Caller::Agent` from `apps/desktop/src-tauri/src/agent/sharing.rs`; it refuses agents unless this setting is on. docs/CLI.md → Share links has the tool's details.

## Where things are stored

| What | Where |
|---|---|
| Endpoint, bucket, region, access key ID, lifetime, cleanup and agent switches | `share-links.json` in Penguin's config folder |
| Secret access key | macOS Keychain, service `co.gluska.penguin.share-links`, account `secret-access-key.v1` |
| Upload records (for cleanup) | `share-uploads.json` in Penguin's data folder |

## For developers

- Backend: `apps/desktop/src-tauri/src/share/` (`config.rs` settings and the Keychain seam, `keys.rs` object names and headers, `s3.rs` signing with [rusty-s3](https://crates.io/crates/rusty-s3) and requests with reqwest, `uploads.rs` records and cleanup, `tests.rs`, and `stub.rs`, an in-process S3 stub the agent and CLI tests share). Agents: `apps/desktop/src-tauri/src/agent/sharing.rs`. Commands: `share_link_config_get/set/clear/test`, `share_file`, `share_delete` (docs/ARCHITECTURE.md).
- UI: `apps/desktop/src/features/share/` (settings page, menu item, upload toast). The mock backend (`src/lib/mock/share.ts`) fakes storage and links on `share.example`, so `npm run dev:mock` shows the whole flow. In the mock, a key ID containing "bad", the secret "wrong" or the bucket "missing" make the test fail.
