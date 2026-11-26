/** The largest and smallest size (px) the dialer's number is shown at. */
export const NUMBER_FONT_MAX = 35;
export const NUMBER_FONT_MIN = 16;
/** The clear button and the spacer that balances it, together. */
const NUMBER_ROW_CHROME = 72;
/** A digit, bracket or dash of the light display face, with the dialer's letter spacing, in em. */
const GLYPH_EM = 0.62;

/**
 * The font size (px) at which `chars` characters of the number fit a row
 * `rowWidth` px wide: the largest size that fits, never below the smallest,
 * so a long number shrinks instead of being cut with an ellipsis. Before the
 * row has been measured (width 0) the largest size is used.
 */
export function numberFontSize(chars: number, rowWidth: number): number {
  const available = rowWidth - NUMBER_ROW_CHROME;
  if (chars === 0 || available <= 0) return NUMBER_FONT_MAX;
  return Math.max(NUMBER_FONT_MIN, Math.min(NUMBER_FONT_MAX, Math.floor(available / (chars * GLYPH_EM))));
}
