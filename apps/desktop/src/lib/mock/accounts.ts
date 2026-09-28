// Provider fields of mock accounts. The real backend derives `capabilities`
// from the provider (penguin-core AccountProvider::capabilities); this is the
// same table for the mock layer only, so keep it in step with types.rs.
import type { AccountProvider, Capabilities, ProviderConfig } from "../types";

const COMMON: Capabilities = {
  labels: false,
  folders: false,
  labelEdit: false,
  labelColors: false,
  inboxCategories: false,
  serverSearch: true,
  windowEstimate: true,
  drafts: true,
  sendLater: true,
  snooze: true,
  calendar: false,
  contactPhotos: false,
  profilePhoto: false,
  messageHeaders: true,
  rawSource: true,
};

export function mockCapabilities(provider: AccountProvider): Capabilities {
  switch (provider) {
    case "gmail":
      return { ...COMMON, labels: true, labelEdit: true, labelColors: true, inboxCategories: true, calendar: true, contactPhotos: true, profilePhoto: true };
    case "imap":
      return { ...COMMON, folders: true };
    case "microsoft":
      return { ...COMMON, labels: true, folders: true, profilePhoto: true };
  }
}

/** The provider fields of a mock account. */
export function mockProviderFields(
  provider: AccountProvider = "gmail",
  providerConfig: ProviderConfig = {},
): { provider: AccountProvider; providerConfig: ProviderConfig; capabilities: Capabilities } {
  return { provider, providerConfig, capabilities: mockCapabilities(provider) };
}
