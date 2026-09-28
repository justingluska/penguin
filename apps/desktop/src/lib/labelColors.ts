// Label colors offered for Gmail labels. Gmail only accepts colors from a
// fixed palette; this list mirrors the backgrounds in
// crates/penguin-gmail/src/label_colors.rs (LABEL_COLORS), which also holds
// each one's text color. Keep the two in lockstep: update_label rejects any
// other background with invalidInput.
export const LABEL_COLORS: { hex: string; name: string }[] = [
  { hex: "#fb4c2f", name: "Red" },
  { hex: "#ffad47", name: "Orange" },
  { hex: "#fad165", name: "Yellow" },
  { hex: "#16a766", name: "Green" },
  { hex: "#43d692", name: "Mint" },
  { hex: "#4a86e8", name: "Blue" },
  { hex: "#c9daf8", name: "Light blue" },
  { hex: "#285bac", name: "Navy" },
  { hex: "#a479e2", name: "Purple" },
  { hex: "#f691b3", name: "Pink" },
  { hex: "#822111", name: "Maroon" },
  { hex: "#999999", name: "Gray" },
];
