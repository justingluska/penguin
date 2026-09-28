// Settings → You photo and the native menu bar, for the mock backend
// (native agent). The photo lives in memory; its id is whatever the handlers
// set on settings.me.photo, as in the real backend.
import type { MockHandler } from "./index";
import { mailHandlers } from "./mail";

// A stand-in "photo": warm backdrop with a head-and-shoulders silhouette.
const MOCK_PHOTO =
  "data:image/svg+xml," +
  encodeURIComponent(
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256"><defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#f6c28b"/><stop offset="1" stop-color="#d9735b"/></linearGradient></defs><rect width="256" height="256" fill="url(#g)"/><circle cx="128" cy="104" r="46" fill="#5b3a2e"/><path d="M40 256c6-58 44-88 88-88s82 30 88 88z" fill="#2f4a6b"/></svg>`,
  );

let photos = new Map<string, string>([["mock-photo", MOCK_PHOTO]]);

function setPhoto(id: string | null) {
  return mailHandlers.update_settings({ patch: { me: { photo: id } } });
}

export const meHandlers: Record<string, MockHandler> = {
  me_photo: async () => {
    const s = (await mailHandlers.get_settings({})) as import("../types").Settings;
    return s.me.photo ? (photos.get(s.me.photo) ?? null) : null;
  },
  set_me_photo: ({ png }) => {
    const id = `mock-${Date.now().toString(16)}`;
    photos = new Map([[id, `data:image/png;base64,${png}`]]);
    return setPhoto(id);
  },
  set_me_photo_from_google: () => {
    photos = new Map([["mock-photo", MOCK_PHOTO]]);
    return setPhoto("mock-photo");
  },
  clear_me_photo: () => setPhoto(null),
  set_menu_context: () => undefined,
  restart_to_update: () => undefined,
  check_for_updates: () => undefined,
  log_client_event: () => undefined,
  read_log: () => ({
    path: "/Users/sam/Library/Logs/co.gluska.penguin/penguin.log",
    truncated: false,
    lines: [
      "2026-09-25T08:01:12.402118Z  INFO penguin_desktop_lib: penguin starting version=\"0.1.0\"",
      "2026-09-25T08:01:13.118004Z  INFO penguin_gmail::sync: incremental sync done account=sam@northwind.example changed=4",
      "2026-09-25T08:03:40.551290Z  WARN penguin_ui: Gmail didn't answer (timed out) source=command what=save_attachment code=network",
      "2026-09-25T08:03:40.553017Z ERROR penguin_ui: Couldn't save the attachment · Gmail didn't answer (timed out) source=toast what=toast code=",
      "2026-09-25T08:05:02.990431Z  WARN penguin_gmail::api: rate limited, backing off account=sam@harbor-labs.example retry_in_ms=2000",
      "2026-09-25T08:07:19.000120Z  INFO penguin_desktop_lib::updater: update found; downloading version=0.1.12",
    ],
  }),
};
