import { expect, it } from 'vitest';
import { columnPins } from './pinning';
const columns = [
  { id: 'queen', width: 420, queen: true, attention: false },
  { id: 'normal', width: 280, queen: false, attention: false },
  { id: 'attention-a', width: 280, queen: false, attention: true },
  { id: 'attention-b', width: 280, queen: false, attention: true },
];
it('G59 pins reserve a full scrolling column and use nonoverlapping cumulative measured offsets', () => {
  expect(columnPins(1100, columns)).toEqual({ queen: 4, 'attention-a': 436 });
  expect(columnPins(1400, columns)).toEqual({ queen: 4, 'attention-a': 436, 'attention-b': 728 });
  expect(columnPins(800, columns)).toEqual({ queen: 4 });
  expect(columnPins(540, columns)).toEqual({});
  expect(columnPins(600, columns)).toEqual({});
  expect(columnPins(1100, columns.map(column => ({ ...column, width: 0 })))).toEqual({});
});
