import { describe, expect, it } from 'vitest';
import { windowChrome } from './windowChrome';

describe('windowChrome', () => {
  it.each([
    ['Mozilla/5.0 (Windows NT 10.0; Win64; x64)', 'custom-controls'],
    ['Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)', 'native-traffic-lights'],
    ['Mozilla/5.0 (X11; Linux x86_64)', 'native-frame'],
    ['Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X)', 'native-frame'],
    ['', 'native-frame'],
  ])('uses %s to select %s', (userAgent, expected) => {
    expect(windowChrome(userAgent)).toBe(expected);
  });
});
