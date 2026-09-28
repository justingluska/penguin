// The Google Cloud part of setup (steps 1–6) and the closing step, as
// numbered instructions with Copy and Open buttons. Matches the console as of
// 2026-09 (Google Auth Platform: Branding, Audience, Clients).
import { useState } from "react";
import { Icon } from "../../components/Icon";
import { Kbd } from "../../components/Kbd";
import { NAMES, links, parseProjectId } from "./links";
import { Callout, Code, CopyButton, Field, OpenButton, Step, Steps, Troubleshooting, Ui } from "./parts";

export interface GuideProps {
  projectId: string;
  setProjectId: (id: string) => void;
}

export function WelcomeStep({ onShortcut }: { onShortcut: () => void }) {
  return (
    <>
      <Callout title="What you need">
        Any Google account and a browser. No billing.
      </Callout>
      <p className="onb-shortcut">
        Already have a Desktop app client JSON?{" "}
        <button className="onb-textbtn" onClick={onShortcut}>
          Skip to adding it <Icon name="right" size="2xs" />
        </button>
      </p>
    </>
  );
}

export function ProjectStep({ projectId, setProjectId }: GuideProps) {
  const [draft, setDraft] = useState(projectId);
  const parsed = parseProjectId(draft);
  const invalid = parsed === null;
  return (
    <>
      <Steps>
        <Step title="Open the new project page" action={<OpenButton url={links.createProject()} primary>Open Google Cloud</OpenButton>}>
          Sign in with the Google account that should own the setup. Any of your accounts works.
        </Step>
        <Step title="Name the project">
          <Field name="Project name">
            <CopyButton text={NAMES.project} label="project name" />
          </Field>
        </Step>
        <Step title="Pick a location, then Create">
          Personal account: leave <Ui>No organization</Ui>. Workspace: pick your organization. Then click <Ui>Create</Ui> and wait for the
          notification (about 20 seconds).
        </Step>
        <Step title="Copy the Project ID (optional, recommended)">
          Google shows it under the name, like <Code>penguin-471523</Code>. Paste it here and every button in the next steps opens this project
          directly.
          <div className="onb-project">
            <label className={`onb-input${invalid ? " is-invalid" : ""}`}>
              <span className="onb-input-label">Project ID</span>
              <input
                value={draft}
                onChange={(e) => {
                  setDraft(e.target.value);
                  const p = parseProjectId(e.target.value);
                  if (p !== null) setProjectId(p);
                }}
                placeholder="penguin-471523"
                spellCheck={false}
                autoCapitalize="off"
                autoCorrect="off"
                aria-invalid={invalid}
              />
              {projectId && !invalid && <Icon name="check" size="xs" className="onb-input-ok" />}
            </label>
            <span className="s2-muted">
              {invalid
                ? "That doesn't look like a Project ID: lowercase letters, digits and dashes (you can paste a console URL, too)."
                : projectId
                  ? `Links now open ${projectId}.`
                  : "Leave it empty to use whichever project the console has selected."}
            </span>
          </div>
        </Step>
      </Steps>
      <Troubleshooting
        problems={[
          {
            symptom: "“You don't have permission to create a project”",
            fix: "Your Workspace admin restricts project creation. Create the project with a personal gmail.com account instead; it still works for your Workspace accounts.",
          },
          {
            symptom: "“Project creation quota exceeded”",
            fix: "Delete a project you no longer use (it's removed after 30 days), or reuse an existing project: just note its Project ID.",
          },
        ]}
      />
    </>
  );
}

export function ApiStep({ projectId }: GuideProps) {
  return (
    <>
      <Steps>
        <Step title="Open the Gmail API page" action={<OpenButton url={links.gmailApi(projectId)} primary>Open Gmail API</OpenButton>}>
          {projectId ? (
            <>
              It opens in <Code>{projectId}</Code>.
            </>
          ) : (
            <>
              Check the project picker at the top left says <Ui>{NAMES.project}</Ui>.
            </>
          )}
        </Step>
        <Step title={<>Click <Ui>Enable</Ui></>}>
          It takes a few seconds and lands on the API's overview page. If Google suggests <Ui>Create credentials</Ui>, ignore it: you'll create
          the client two steps from now.
        </Step>
      </Steps>
      <Callout tone="ok" title="The Gmail API is free">
        Google doesn't charge for it and doesn't need billing turned on.
      </Callout>
      <Troubleshooting
        problems={[
          { symptom: "There's a Manage button instead of Enable", fix: "It's already enabled. Move on." },
          {
            symptom: "Later: “Gmail API has not been used in project …” (accessNotConfigured)",
            fix: "The API was enabled in a different project than the client JSON came from. Enable it in the project that owns the client.",
          },
        ]}
      />
    </>
  );
}

export function BrandingStep({ projectId }: GuideProps) {
  return (
    <>
      <Steps>
        <Step title="Open Google Auth Platform" action={<OpenButton url={links.branding(projectId)} primary>Open Branding</OpenButton>}>
          If it says <Ui>Google Auth Platform not configured yet</Ui>, click <Ui>Get started</Ui>.
        </Step>
        <Step title="App Information">
          <Field name="App name">
            <CopyButton text={NAMES.app} label="app name" />
          </Field>
          <Field name="User support email">your email address (pick it from the list)</Field>
          Then <Ui>Next</Ui>.
          <Callout tone="warn" title="Don't upload a logo">
            A logo makes Google require brand verification (a review that can take weeks) before anyone can sign in. Penguin doesn't need one.
          </Callout>
        </Step>
        <Step title={<>Audience: choose <Ui>External</Ui></>}>
          <div className="onb-choice">
            <div className="onb-choice-card is-pick">
              <b>External</b>
              <span>Works for gmail.com and for accounts in any Workspace organization. Pick this.</span>
            </div>
            <div className="onb-choice-card">
              <b>Internal</b>
              <span>Only if every account you'll add is in this one Workspace organization.</span>
            </div>
          </div>
        </Step>
        <Step title="Contact Information">
          Your email address, then <Ui>Next</Ui>.
        </Step>
        <Step title="Finish">
          Tick <Ui>I agree to the Google API Services: User Data Policy</Ui>, then <Ui>Continue</Ui> and <Ui>Create</Ui>.
        </Step>
      </Steps>
      <Troubleshooting
        problems={[
          {
            symptom: "Chose Internal by mistake",
            fix: (
              <>
                Open <Ui>Audience</Ui> and click <Ui>Make external</Ui>. Otherwise accounts outside the organization see <Code>org_internal</Code>.
              </>
            ),
          },
          {
            symptom: "Scopes / Data Access",
            fix: "Nothing to add there. Penguin asks for Gmail access itself when you sign in.",
          },
        ]}
      />
    </>
  );
}

export function PublishStep({ projectId }: GuideProps) {
  return (
    <>
      <Steps>
        <Step title="Open Audience" action={<OpenButton url={links.audience(projectId)} primary>Open Audience</OpenButton>}>
          Under <Ui>Publishing status</Ui> it says <Ui>Testing</Ui>.
        </Step>
        <Step title={<>Click <Ui>Publish app</Ui>, then <Ui>Confirm</Ui></>}>
          The status changes to <Ui>In production</Ui>. You can skip the test users list.
        </Step>
      </Steps>
      <Callout tone="warn" title="Why this matters">
        While an app is in Testing, Google signs every account out after 7 days. Publishing turns that off.
      </Callout>
      <Callout title="Publishing doesn't make anything public">
        It doesn't list Penguin anywhere or share your mail. You don't need to submit it for verification either: a personal app with under 100
        users can stay unverified, so Google shows an "unverified app" notice when you sign in. The “Sign in” step shows you how to get past it.
      </Callout>
      <Troubleshooting
        problems={[
          { symptom: "No Publish app button", fix: "The audience is Internal. Internal apps don't expire, so there's nothing to publish." },
          {
            symptom: (
              <>
                Accounts need signing in again after a week (<Code>invalid_grant</Code>)
              </>
            ),
            fix: "The app was still in Testing when they signed in. Publish it, then use Reconnect on each account in Settings → Accounts.",
          },
        ]}
      />
    </>
  );
}

export function ClientStep({ projectId }: GuideProps) {
  return (
    <>
      <Steps>
        <Step title="Open Create client" action={<OpenButton url={links.createClient(projectId)} primary>Open Clients</OpenButton>} />
        <Step title={<>Application type: <Ui>Desktop app</Ui></>}>Not Web application. Desktop apps need no redirect URIs.</Step>
        <Step title="Name it, then Create">
          <Field name="Name">
            <CopyButton text={NAMES.client} label="client name" />
          </Field>
        </Step>
        <Step title={<>Click <Ui>Download JSON</Ui></>}>
          In the <Ui>OAuth client created</Ui> dialog. The file lands in Downloads as <Code>client_secret_….apps.googleusercontent.com.json</Code>.
        </Step>
      </Steps>
      <Callout tone="warn" title="Download it before closing the dialog">
        Google shows the full client secret only once. Closed it already? Create another Desktop app client: it's free and takes ten seconds.
      </Callout>
      <Troubleshooting
        problems={[
          {
            symptom: "Created a Web application client",
            fix: "Penguin can't use it. Create a new client with Application type Desktop app.",
          },
          { symptom: "It asks for a redirect URI", fix: "That's the Web application form. Switch Application type to Desktop app." },
          {
            symptom: "Keep the file private",
            fix: "It identifies your app to Google. Don't post it or commit it to a repository. Penguin copies it into its own settings folder.",
          },
        ]}
      />
    </>
  );
}

/** `gmail`: the account just added is Google, so the Gmail quota tip applies. */
export function DoneStep({ projectId, gmail = true }: GuideProps & { gmail?: boolean }) {
  return (
    <>
      <p className="onb-lead onb-lead-first">Syncing now, newest mail first.</p>
      <section className="onb-keys" aria-label="Keyboard basics">
        {(
          [
            ["j", "k", "Move down / up"],
            ["↵", null, "Open a conversation"],
            ["e", null, "Done (archive)"],
            ["c", null, "Compose"],
            ["/", null, "Search everything"],
            ["⌘", "K", "Command menu"],
            ["?", null, "All shortcuts"],
            ["⌘", ",", "Settings"],
          ] as const
        ).map(([a, b, label]) => (
          <div className="onb-key" key={label}>
            <span className="kbd-group">
              <Kbd>{a}</Kbd>
              {b && <Kbd>{b}</Kbd>}
            </span>
            <span>{label}</span>
          </div>
        ))}
      </section>
      {gmail && (
        <details className="onb-trouble onb-quota">
          <summary>
            <Icon name="right" size="xs" className="onb-trouble-chev" />
            Optional: sync big mailboxes faster
          </summary>
          <div className="onb-quota-body">
            <p>
              Google lets each account use 6,000 Gmail API units a minute. That's about <b>100 emails a minute</b> per account while Penguin
              downloads your history (a 50,000-email mailbox takes about 8 hours). New mail always arrives right away.
            </p>
            <p>To ask Google for more:</p>
            <ol>
              <li>
                Link a billing account to the project{" "}
                <OpenButton url={links.billing(projectId)}>Billing</OpenButton>. The Gmail API stays free; Google just requires one for quota
                increases.
              </li>
              <li>
                Open the Gmail API quotas <OpenButton url={links.quotas(projectId)}>Quotas</OpenButton>, find <Ui>Units per minute per user</Ui>,
                choose <Ui>Edit quota</Ui> and request a higher value.
              </li>
              <li>New projects have to wait about 48 hours before Google accepts a request.</li>
            </ol>
            <p className="s2-muted">
              Penguin paces itself to the default. Once Google approves more, enter the new limit in Settings → Developer → <Ui>Gmail quota</Ui>.
              It applies right away.
            </p>
          </div>
        </details>
      )}
    </>
  );
}
