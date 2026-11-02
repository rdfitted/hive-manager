export function classifyFinding(name, passed, details, known = {}) {
  const finding = known[name];
  if (!finding) return { passed: !!passed, accepted: !!passed, status: passed ? 'pass' : 'fail' };
  if (!Number.isInteger(finding.issue) || finding.issue <= 0) return { passed: !!passed, accepted: false, status: 'fail', reason: 'Known finding has no issue number' };
  if (passed) return { passed: true, accepted: false, status: 'unexpected-pass', issue: finding.issue, reason: 'Known finding started passing; reconcile the recorded issue instead of silently dropping it' };
  const box = details?.composer, pane = details?.pane;
  // Observed bottom overflow: 5.61–103.61 CSS px at the two widths.
  // 120px allows layout/font variance without accepting a different defect.
  const validRect = rect => rect && ['x', 'y', 'width', 'height', 'right', 'bottom'].every(key => Number.isFinite(rect[key])) && rect.width > 0 && rect.height > 0 && Math.abs(rect.right - rect.x - rect.width) < 1 && Math.abs(rect.bottom - rect.y - rect.height) < 1;
  const overflow = validRect(box) && validRect(pane) && box.x >= pane.x - 1 && box.right <= pane.right + 1 && box.y >= pane.y - 1 && box.y < pane.bottom && box.bottom > pane.bottom + 1 && box.bottom - pane.bottom <= 120;
  const expectedCard = name === 'grid-composer-with-attention';
  if (finding.issue !== 414 || !['grid-composer-with-attention', 'grid-composer-without-attention'].includes(name) || !overflow || details.contained !== false || details.card !== expectedCard) return { passed: false, accepted: false, status: 'fail', issue: finding.issue, reason: 'Failure does not match the recorded bottom-only composer-overflow finding (maximum 120 CSS px)' };
  return { passed: false, accepted: true, status: 'known-finding', issue: finding.issue };
}
