// Right-click menu for a person (names in thread headers, search's People
// group, compose chips). OWNER: menus agent.
import type { Address } from "../../lib/types";
import { api } from "../../lib/api";
import { setUi } from "../../lib/ui";
import { displayName } from "../../lib/format";
import { copyText } from "../../lib/clipboard";
import type { MenuEntries } from "../../components/ContextMenu";
import { isMe } from "../../app/store";
import { openComposeTo } from "../compose";
import { openPersonCard } from "./PersonCard";

export function personMenu(person: Address, anchor?: HTMLElement | null): MenuEntries {
  const me = isMe(person.email);
  const named = person.name?.trim() ? `${person.name.trim()} <${person.email}>` : person.email;
  return [
    { label: "Open person card", icon: "user", onSelect: () => openPersonCard(person, anchor ?? null) },
    !me && { label: `New message to ${displayName(person)}`, icon: "compose", onSelect: () => void openComposeTo(person) },
    {
      label: "Search all mail with them",
      icon: "search",
      onSelect: () => setUi({ overlay: "search", searchPrefill: `from:${person.email} OR to:${person.email}` }),
    },
    { type: "separator" },
    { label: "Copy email address", icon: "copy", onSelect: () => void copyText(person.email, "Email copied") },
    named !== person.email && { label: "Copy name and email", icon: "copy", onSelect: () => void copyText(named, "Copied") },
    !me && {
      label: "Find in Google Contacts",
      icon: "external",
      onSelect: () => void api.openExternal(`https://contacts.google.com/search/${encodeURIComponent(person.email)}`),
    },
  ];
}
