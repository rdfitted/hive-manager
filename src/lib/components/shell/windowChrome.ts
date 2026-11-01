export type WindowChrome = 'custom-controls' | 'native-traffic-lights' | 'native-frame';

/** Match the platform overrides without requiring a native call during rendering. */
export function windowChrome(userAgent: string): WindowChrome {
  if (/Windows/i.test(userAgent)) return 'custom-controls';
  if (/Macintosh|Mac OS X/i.test(userAgent) && !/iPhone|iPad|iPod/i.test(userAgent)) {
    return 'native-traffic-lights';
  }
  return 'native-frame';
}
