import { describe, expect, it } from 'vitest';
import { GradingStatus } from '../../api/types';
import {
  matchesStatusFilter,
  parseStatusFilter,
  STATUS_FILTER_OPTIONS,
  statusFilterLabel,
  statusFilterOf,
  statusWords,
} from '../statusFilter';

describe('status filter', () => {
  it('matches each word against its grade and nothing else', () => {
    expect(matchesStatusFilter('accepted', GradingStatus.Accepted)).toBe(true);
    expect(matchesStatusFilter('accepted', GradingStatus.Pending)).toBe(false);
    expect(matchesStatusFilter('rejected', GradingStatus.Rejected)).toBe(true);
    expect(matchesStatusFilter('pending', GradingStatus.Pending)).toBe(true);
    expect(matchesStatusFilter('pending', GradingStatus.Rejected)).toBe(false);
  });

  it('lets everything through for All and for anything it cannot read', () => {
    for (const grade of [GradingStatus.Pending, GradingStatus.Accepted, GradingStatus.Rejected]) {
      expect(matchesStatusFilter('all', grade)).toBe(true);
      expect(matchesStatusFilter('', grade)).toBe(true);
      expect(matchesStatusFilter('bogus', grade)).toBe(true);
    }
  });

  it('still reads the grade numbers older links carried', () => {
    // The select used to emit "1" for Accepted; a bookmark with ?status=1
    // must keep meaning Accepted rather than silently turning into All.
    expect(parseStatusFilter('1')).toBe('accepted');
    expect(parseStatusFilter('2')).toBe('rejected');
    expect(parseStatusFilter('0')).toBe('pending');
    expect(matchesStatusFilter('1', GradingStatus.Accepted)).toBe(true);
    expect(matchesStatusFilter('1', GradingStatus.Pending)).toBe(false);
  });

  it('is case- and whitespace-tolerant and labels every option', () => {
    expect(parseStatusFilter(' Accepted ')).toBe('accepted');
    expect(statusFilterLabel('accepted')).toBe('Accepted');
    expect(statusFilterLabel('1')).toBe('Accepted');
    expect(statusFilterLabel(null as unknown as string)).toBe('All');
    expect(STATUS_FILTER_OPTIONS.map((option) => option.value)).toEqual([
      'accepted',
      'rejected',
      'pending',
    ]);
  });

  it('keeps any mix of grades, in one order, and every grade is All', () => {
    // Everything but rejected.
    expect(parseStatusFilter('pending,accepted')).toBe('accepted,pending');
    expect(matchesStatusFilter('accepted,pending', GradingStatus.Accepted)).toBe(true);
    expect(matchesStatusFilter('accepted,pending', GradingStatus.Pending)).toBe(true);
    expect(matchesStatusFilter('accepted,pending', GradingStatus.Rejected)).toBe(false);
    expect(statusFilterLabel('accepted,pending')).toBe('Accepted and Pending');
    expect(parseStatusFilter('accepted,rejected,pending')).toBe('all');
    expect(parseStatusFilter('1,0')).toBe('accepted,pending');
    expect(parseStatusFilter('rejected,bogus')).toBe('rejected');
    expect(statusFilterOf([])).toBe('all');
    expect(statusWords('all')).toEqual(['accepted', 'rejected', 'pending']);
  });
});
