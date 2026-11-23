import { describe, expect, it } from 'vitest';
import { DEFAULTS, fromStored, toStored } from './settings';

describe('saved settings', () => {
  it('stores nothing for the defaults', () => {
    expect(toStored('sizeMode', 'disk')).toEqual(['SizeMode', null]);
    expect(toStored('pane', true)).toEqual(['DetailsPane', null]);
  });

  it('stores what differs', () => {
    expect(toStored('sizeMode', 'logical')).toEqual(['SizeMode', 'logical']);
    expect(toStored('pane', false)).toEqual(['DetailsPane', 0]);
  });

  it('reads back what it stored', () => {
    expect(fromStored({ SizeMode: 'logical', DetailsPane: 0 })).toEqual({ sizeMode: 'logical', pane: false });
    expect(fromStored({})).toEqual(DEFAULTS);
  });

  it('ignores what it does not understand', () => {
    expect(fromStored({ SizeMode: 'huge', DetailsPane: 7, Other: 1 })).toEqual(DEFAULTS);
    expect(fromStored({ SizeMode: 1, DetailsPane: '0' })).toEqual(DEFAULTS);
  });
});
