export type CssTokenName = `--${string}`;

type ResolvedTokens<T extends Record<string, CssTokenName>> = {
  [K in keyof T]: string;
};

interface Rgb {
  r: number;
  g: number;
  b: number;
}

interface OKLab {
  l: number;
  a: number;
  b: number;
}

const HEX_COLOR_PATTERN = /^#([\da-f]{3}|[\da-f]{6})$/i;

/**
 * Resolve a named batch of CSS custom properties once at browser runtime.
 *
 * CSSOM returns an empty string for both missing and explicitly empty custom
 * properties, so both cases deliberately fail together instead of falling
 * through to an invalid xterm colour.
 */
export function resolveTokens<const T extends Record<string, CssTokenName>>(
  tokens: T,
): ResolvedTokens<T> {
  if (typeof document === 'undefined' || typeof getComputedStyle === 'undefined') {
    throw new Error('resolveTokens is browser-only and must be called after mount');
  }

  const invalidNames = Object.entries(tokens)
    .filter(([, token]) => !token.startsWith('--'))
    .map(([key, token]) => `${key} (${token})`);

  if (invalidNames.length > 0) {
    throw new Error(`Invalid CSS token name(s): ${invalidNames.join(', ')}`);
  }

  const styles = getComputedStyle(document.documentElement);
  const resolvedEntries: Array<[string, string]> = [];
  const unresolved: string[] = [];

  for (const [key, token] of Object.entries(tokens)) {
    const value = styles.getPropertyValue(token).trim();
    if (!value) {
      unresolved.push(`${key} (${token})`);
      continue;
    }
    resolvedEntries.push([key, value]);
  }

  if (unresolved.length > 0) {
    throw new Error(`Missing or empty CSS token(s): ${unresolved.join(', ')}`);
  }

  return Object.fromEntries(resolvedEntries) as ResolvedTokens<T>;
}

/**
 * Mix two opaque sRGB hex colours by interpolating their Cartesian OKLab
 * coordinates. Terminal palette inputs are deliberately defined as hex tokens.
 */
export function mixOKLab(first: string, second: string, shareOfSecond: number): string {
  assertUnitInterval(shareOfSecond, 'shareOfSecond');

  const a = rgbToOKLab(parseHexColor(first));
  const b = rgbToOKLab(parseHexColor(second));
  const mixed: OKLab = {
    l: a.l + (b.l - a.l) * shareOfSecond,
    a: a.a + (b.a - a.a) * shareOfSecond,
    b: a.b + (b.b - a.b) * shareOfSecond,
  };

  return rgbToHex(okLabToRgb(mixed));
}

/** Return a concrete rgba() string while preserving a resolved token's RGB. */
export function withAlpha(color: string, alpha: number): string {
  assertUnitInterval(alpha, 'alpha');
  const { r, g, b } = parseHexColor(color);
  return `rgba(${r}, ${g}, ${b}, ${alpha})`;
}

function assertUnitInterval(value: number, label: string): void {
  if (!Number.isFinite(value) || value < 0 || value > 1) {
    throw new RangeError(`${label} must be a finite number between 0 and 1`);
  }
}

function parseHexColor(value: string): Rgb {
  const match = HEX_COLOR_PATTERN.exec(value.trim());
  if (!match) {
    throw new Error(`Expected an opaque sRGB hex colour, received "${value}"`);
  }

  const digits = match[1].length === 3
    ? [...match[1]].map((digit) => digit + digit).join('')
    : match[1];

  return {
    r: Number.parseInt(digits.slice(0, 2), 16),
    g: Number.parseInt(digits.slice(2, 4), 16),
    b: Number.parseInt(digits.slice(4, 6), 16),
  };
}

function rgbToOKLab({ r, g, b }: Rgb): OKLab {
  const red = srgbToLinear(r / 255);
  const green = srgbToLinear(g / 255);
  const blue = srgbToLinear(b / 255);

  const l = Math.cbrt(0.4122214708 * red + 0.5363325363 * green + 0.0514459929 * blue);
  const m = Math.cbrt(0.2119034982 * red + 0.6806995451 * green + 0.1073969566 * blue);
  const s = Math.cbrt(0.0883024619 * red + 0.2817188376 * green + 0.6299787005 * blue);

  return {
    l: 0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s,
    a: 1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s,
    b: 0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s,
  };
}

function okLabToRgb({ l, a, b }: OKLab): Rgb {
  const lPrime = l + 0.3963377774 * a + 0.2158037573 * b;
  const mPrime = l - 0.1055613458 * a - 0.0638541728 * b;
  const sPrime = l - 0.0894841775 * a - 1.291485548 * b;

  const lCube = lPrime ** 3;
  const mCube = mPrime ** 3;
  const sCube = sPrime ** 3;

  const red = 4.0767416621 * lCube - 3.3077115913 * mCube + 0.2309699292 * sCube;
  const green = -1.2684380046 * lCube + 2.6097574011 * mCube - 0.3413193965 * sCube;
  const blue = -0.0041960863 * lCube - 0.7034186147 * mCube + 1.707614701 * sCube;

  return {
    r: toByte(linearToSrgb(red)),
    g: toByte(linearToSrgb(green)),
    b: toByte(linearToSrgb(blue)),
  };
}

function srgbToLinear(value: number): number {
  return value <= 0.04045
    ? value / 12.92
    : ((value + 0.055) / 1.055) ** 2.4;
}

function linearToSrgb(value: number): number {
  return value <= 0.0031308
    ? 12.92 * value
    : 1.055 * value ** (1 / 2.4) - 0.055;
}

function toByte(value: number): number {
  return Math.round(Math.max(0, Math.min(1, value)) * 255);
}

function rgbToHex({ r, g, b }: Rgb): string {
  return `#${[r, g, b]
    .map((channel) => channel.toString(16).padStart(2, '0'))
    .join('')
    .toUpperCase()}`;
}
