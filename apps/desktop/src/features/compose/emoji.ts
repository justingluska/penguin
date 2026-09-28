// Emoji shortcodes for ":smi" autocomplete in the composer. A small local
// list (GitHub/Slack-style names), so nothing is fetched. Order is rough
// popularity: it breaks ties between equally good prefix matches.

export const EMOJI: Array<[string, string]> = [
  ["smile", "😄"], ["slightly_smiling_face", "🙂"], ["grinning", "😀"], ["joy", "😂"], ["rofl", "🤣"],
  ["wink", "😉"], ["blush", "😊"], ["heart_eyes", "😍"], ["star_struck", "🤩"], ["thinking", "🤔"],
  ["neutral_face", "😐"], ["expressionless", "😑"], ["roll_eyes", "🙄"], ["smirk", "😏"], ["relieved", "😌"],
  ["pensive", "😔"], ["sweat_smile", "😅"], ["laughing", "😆"], ["upside_down", "🙃"], ["innocent", "😇"],
  ["sunglasses", "😎"], ["nerd", "🤓"], ["hugs", "🤗"], ["shushing", "🤫"], ["zipper_mouth", "🤐"],
  ["raised_eyebrow", "🤨"], ["grimacing", "😬"], ["cry", "😢"], ["sob", "😭"], ["disappointed", "😞"],
  ["worried", "😟"], ["confused", "😕"], ["frowning", "☹️"], ["angry", "😠"], ["rage", "😡"],
  ["scream", "😱"], ["flushed", "😳"], ["sleeping", "😴"], ["yawning", "🥱"], ["mask", "😷"],
  ["partying_face", "🥳"], ["pleading", "🥺"], ["exploding_head", "🤯"], ["cold_sweat", "😰"], ["hot_face", "🥵"],
  ["saluting_face", "🫡"], ["melting_face", "🫠"], ["face_palm", "🤦"], ["shrug", "🤷"], ["see_no_evil", "🙈"],
  ["thumbsup", "👍"], ["+1", "👍"], ["thumbsdown", "👎"], ["-1", "👎"], ["ok_hand", "👌"],
  ["clap", "👏"], ["raised_hands", "🙌"], ["pray", "🙏"], ["wave", "👋"], ["muscle", "💪"],
  ["point_up", "☝️"], ["point_right", "👉"], ["point_left", "👈"], ["point_down", "👇"], ["v", "✌️"],
  ["crossed_fingers", "🤞"], ["handshake", "🤝"], ["writing_hand", "✍️"], ["eyes", "👀"], ["brain", "🧠"],
  ["heart", "❤️"], ["orange_heart", "🧡"], ["yellow_heart", "💛"], ["green_heart", "💚"], ["blue_heart", "💙"],
  ["purple_heart", "💜"], ["black_heart", "🖤"], ["broken_heart", "💔"], ["sparkling_heart", "💖"], ["100", "💯"],
  ["fire", "🔥"], ["sparkles", "✨"], ["star", "⭐"], ["zap", "⚡"], ["boom", "💥"],
  ["tada", "🎉"], ["confetti_ball", "🎊"], ["balloon", "🎈"], ["gift", "🎁"], ["trophy", "🏆"],
  ["medal", "🏅"], ["rocket", "🚀"], ["dart", "🎯"], ["bulb", "💡"], ["memo", "📝"],
  ["pencil", "✏️"], ["pushpin", "📌"], ["paperclip", "📎"], ["link", "🔗"], ["lock", "🔒"],
  ["key", "🔑"], ["calendar", "📅"], ["date", "📆"], ["clock", "🕐"], ["hourglass", "⏳"],
  ["alarm_clock", "⏰"], ["email", "📧"], ["envelope", "✉️"], ["inbox_tray", "📥"], ["outbox_tray", "📤"],
  ["package", "📦"], ["chart", "📈"], ["chart_down", "📉"], ["bar_chart", "📊"], ["clipboard", "📋"],
  ["file_folder", "📁"], ["page_facing_up", "📄"], ["books", "📚"], ["computer", "💻"], ["phone", "📱"],
  ["telephone", "☎️"], ["camera", "📷"], ["movie_camera", "🎥"], ["mag", "🔍"], ["gear", "⚙️"],
  ["hammer", "🔨"], ["wrench", "🔧"], ["toolbox", "🧰"], ["money", "💰"], ["dollar", "💵"],
  ["credit_card", "💳"], ["briefcase", "💼"], ["office", "🏢"], ["house", "🏠"], ["car", "🚗"],
  ["airplane", "✈️"], ["train", "🚆"], ["globe", "🌍"], ["world_map", "🗺️"], ["beach", "🏖️"],
  ["sunny", "☀️"], ["cloud", "☁️"], ["rain", "🌧️"], ["snowflake", "❄️"], ["rainbow", "🌈"],
  ["coffee", "☕"], ["tea", "🍵"], ["beer", "🍺"], ["wine", "🍷"], ["champagne", "🍾"],
  ["pizza", "🍕"], ["cake", "🍰"], ["birthday", "🎂"], ["cookie", "🍪"], ["apple", "🍎"],
  ["dog", "🐶"], ["cat", "🐱"], ["penguin", "🐧"], ["unicorn", "🦄"], ["bee", "🐝"],
  ["seedling", "🌱"], ["evergreen_tree", "🌲"], ["sunflower", "🌻"], ["rose", "🌹"], ["four_leaf_clover", "🍀"],
  ["white_check_mark", "✅"], ["check", "✔️"], ["x", "❌"], ["warning", "⚠️"], ["no_entry", "⛔"],
  ["question", "❓"], ["exclamation", "❗"], ["heavy_plus_sign", "➕"], ["arrow_right", "➡️"], ["arrow_left", "⬅️"],
  ["arrow_up", "⬆️"], ["arrow_down", "⬇️"], ["repeat", "🔁"], ["hourglass_done", "⌛"], ["red_circle", "🔴"],
  ["green_circle", "🟢"], ["yellow_circle", "🟡"], ["blue_circle", "🔵"], ["speech_balloon", "💬"], ["thought_balloon", "💭"],
  ["zzz", "💤"], ["wave_dash", "〰️"], ["infinity", "♾️"], ["copyright", "©️"], ["tm", "™️"],
];

/** Prefix matches first (on the name or on a word inside it), then substrings. */
export function matchEmoji(typed: string, limit = 8): Array<[string, string]> {
  const t = typed.toLowerCase();
  const seen = new Set<string>();
  const out: Array<[string, string]> = [];
  const push = (e: [string, string]) => {
    // "+1" and "thumbsup" are the same glyph: show it once.
    if (seen.has(e[1]) || out.length >= limit) return;
    seen.add(e[1]);
    out.push(e);
  };
  for (const e of EMOJI) if (e[0].startsWith(t)) push(e);
  for (const e of EMOJI) if (e[0].split("_").some((w) => w.startsWith(t))) push(e);
  if (t.length >= 3) for (const e of EMOJI) if (e[0].includes(t)) push(e);
  return out;
}
