// Small fuzzy matcher for the palette: every query character must appear in
// order. Consecutive runs, word starts and an early first hit score higher.
// Returns 0 for no match.

export function fuzzyScore(query: string, text: string): number {
  const q = query.toLowerCase();
  const t = text.toLowerCase();
  if (!q) return 1;
  const direct = t.indexOf(q);
  if (direct !== -1) {
    // Word-start substrings rank highest; a short mid-word hit ("mi" in
    // "coming") is barely a match.
    const atWord = direct === 0 || /[\s\-_/]/.test(t[direct - 1]);
    return (atWord ? 140 : q.length >= 3 ? 60 : 12) - direct * 0.5 - (t.length - q.length) * 0.05;
  }
  let score = 0;
  let ti = 0;
  let run = 0;
  for (let qi = 0; qi < q.length; qi++) {
    const c = q[qi];
    if (c === " ") continue;
    const found = t.indexOf(c, ti);
    if (found === -1) return 0;
    const atWord = found === 0 || /[\s\-_/]/.test(t[found - 1]);
    run = found === ti ? run + 1 : 0;
    score += 1 + run * 2 + (atWord ? 4 : 0);
    ti = found + 1;
  }
  // Scattered hits must mostly land on word starts or runs, or it's noise.
  return score >= q.replace(/ /g, "").length * 3 ? score : 0;
}
