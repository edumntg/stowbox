/** Fuzzy matching for the command palette. Pure: no Solid, no DOM.
 *
 * The query is split on spaces and every word has to match, in the label or
 * in the item's keywords. A word matches as a substring (best at the start
 * of the label or of a word) or, failing that, as letters in order. */

export interface Match {
  score: number;
  /** Character positions in the label that matched, for highlighting. */
  indices: number[];
}

const isBoundary = (text: string, i: number) => i === 0 || /[\s\-_./:·]/.test(text[i - 1]) || (/[a-z]/.test(text[i - 1]) && /[A-Z]/.test(text[i]));

/** Scores one word against one text; null when it does not match. */
export function matchWord(word: string, text: string): Match | null {
  if (!word) return { score: 0, indices: [] };
  const lower = text.toLowerCase();
  const w = word.toLowerCase();
  // Substring: prefer one at a word start over the first one.
  let best = -1;
  for (let i = lower.indexOf(w); i >= 0; i = lower.indexOf(w, i + 1)) {
    if (best < 0) best = i;
    if (isBoundary(text, i)) {
      best = i;
      break;
    }
  }
  if (best >= 0) {
    const bonus = best === 0 ? 60 : isBoundary(text, best) ? 40 : 0;
    return { score: 100 + bonus + w.length * 4 - Math.min(best, 30), indices: Array.from({ length: w.length }, (_, k) => best + k) };
  }
  // Letters in order, greedy, rewarding word starts and runs.
  const indices: number[] = [];
  let score = 0;
  let from = 0;
  for (const ch of w) {
    if (ch === " ") continue;
    // Take the next word start that has this letter, if it comes before
    // the next plain occurrence would leave letters unmatched; else the next.
    let at = -1;
    for (let i = from; i < lower.length; i++) {
      if (lower[i] === ch && isBoundary(text, i)) {
        at = i;
        break;
      }
    }
    const plain = lower.indexOf(ch, from);
    if (plain < 0) return null;
    if (at < 0 || at - plain > 8) at = plain;
    score += 10;
    if (isBoundary(text, at)) score += 8;
    if (indices.length && indices[indices.length - 1] === at - 1) score += 6;
    score -= Math.min(at - from, 10) * 0.5;
    indices.push(at);
    from = at + 1;
  }
  // Letters scattered over the whole label are noise, not a match.
  const span = indices[indices.length - 1] - indices[0] + 1;
  if (span > Math.max(indices.length * 2, indices.length + 4)) return null;
  return { score, indices };
}

/** Scores a whole query against a label and optional keywords. Keyword
 * matches count for less and are not highlighted. */
export function matchItem(query: string, label: string, keywords = ""): Match | null {
  const words = query.trim().split(/\s+/).filter(Boolean);
  if (!words.length) return { score: 0, indices: [] };
  let score = 0;
  const indices = new Set<number>();
  for (const word of words) {
    const inLabel = matchWord(word, label);
    const inKeys = keywords ? matchWord(word, keywords) : null;
    if (inLabel && (!inKeys || inLabel.score >= inKeys.score * 0.6)) {
      score += inLabel.score;
      inLabel.indices.forEach((i) => indices.add(i));
    } else if (inKeys) {
      score += inKeys.score * 0.6;
    } else {
      return null;
    }
  }
  // Shorter labels win ties: "Fleet" over "Fleet overview settings".
  score -= label.length * 0.1;
  return { score, indices: [...indices].sort((a, b) => a - b) };
}

/** The items that match, best first; ties keep their original order. */
export function rank<T>(items: T[], query: string, text: (item: T) => { label: string; keywords?: string }): { item: T; match: Match }[] {
  const out: { item: T; match: Match; order: number }[] = [];
  items.forEach((item, order) => {
    const t = text(item);
    const match = matchItem(query, t.label, t.keywords);
    if (match) out.push({ item, match, order });
  });
  if (query.trim()) out.sort((a, b) => b.match.score - a.match.score || a.order - b.order);
  return out.map(({ item, match }) => ({ item, match }));
}

/** Splits a label into runs for rendering, marking the matched characters. */
export function highlight(label: string, indices: number[]): { text: string; hit: boolean }[] {
  const set = new Set(indices);
  const runs: { text: string; hit: boolean }[] = [];
  for (let i = 0; i < label.length; i++) {
    const hit = set.has(i);
    const last = runs[runs.length - 1];
    if (last && last.hit === hit) last.text += label[i];
    else runs.push({ text: label[i], hit });
  }
  return runs;
}
