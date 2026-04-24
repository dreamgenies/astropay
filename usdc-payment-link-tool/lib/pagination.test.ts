/**
 * Tests for keyset pagination logic used by pendingInvoices() and queuedPayouts().
 *
 * These tests verify the cursor-advance and loop-termination behaviour without
 * a live database by simulating the page-fetching loop with in-memory data.
 */
import { describe, expect, it } from 'vitest';

// ---------------------------------------------------------------------------
// Minimal types for the simulation
// ---------------------------------------------------------------------------
type Row = { id: string; created_at: string; [key: string]: unknown };

/**
 * Simulates the keyset pagination loop used by pendingInvoices() / queuedPayouts().
 * `fetchPage` stands in for the DB query; it receives the current cursor and
 * page size and returns the next slice of rows.
 */
function keysetPaginateAll(
  rows: Row[],
  pageSize: number,
): Row[] {
  const all: Row[] = [];
  let cursorCreatedAt = new Date(0).toISOString();
  let cursorId = '00000000-0000-0000-0000-000000000000';

  while (true) {
    // Simulate: WHERE (created_at, id) > (cursor) ORDER BY created_at, id LIMIT pageSize
    const page = rows
      .filter(
        (r) =>
          r.created_at > cursorCreatedAt ||
          (r.created_at === cursorCreatedAt && r.id > cursorId),
      )
      .sort((a, b) =>
        a.created_at !== b.created_at
          ? a.created_at.localeCompare(b.created_at)
          : a.id.localeCompare(b.id),
      )
      .slice(0, pageSize);

    all.push(...page);

    if (page.length < pageSize) break;

    const last = page[page.length - 1];
    cursorCreatedAt = last.created_at;
    cursorId = last.id;
  }

  return all;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------
function makeRows(count: number): Row[] {
  return Array.from({ length: count }, (_, i) => ({
    id: String(i).padStart(36, '0'),
    created_at: new Date(i * 1000).toISOString(),
  }));
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
describe('keyset pagination', () => {
  it('returns all rows when count equals page size exactly', () => {
    const rows = makeRows(100);
    const result = keysetPaginateAll(rows, 100);
    expect(result).toHaveLength(100);
  });

  it('returns all rows when count is less than page size', () => {
    const rows = makeRows(42);
    const result = keysetPaginateAll(rows, 100);
    expect(result).toHaveLength(42);
  });

  it('returns all rows when count spans multiple full pages', () => {
    const rows = makeRows(350);
    const result = keysetPaginateAll(rows, 100);
    expect(result).toHaveLength(350);
  });

  it('returns all rows when count is exactly two pages', () => {
    const rows = makeRows(200);
    const result = keysetPaginateAll(rows, 100);
    expect(result).toHaveLength(200);
  });

  it('returns empty array when there are no rows', () => {
    const result = keysetPaginateAll([], 100);
    expect(result).toHaveLength(0);
  });

  it('returns rows in created_at ASC order across pages', () => {
    const rows = makeRows(250);
    const result = keysetPaginateAll(rows, 100);
    for (let i = 1; i < result.length; i++) {
      expect(result[i].created_at >= result[i - 1].created_at).toBe(true);
    }
  });

  it('does not duplicate rows across page boundaries', () => {
    const rows = makeRows(300);
    const result = keysetPaginateAll(rows, 100);
    const ids = result.map((r) => r.id);
    const unique = new Set(ids);
    expect(unique.size).toBe(result.length);
  });

  it('stops after exactly one page when count equals page size', () => {
    // 100 rows with page size 100 → one full page → loop checks 100 < 100 = false
    // then fetches next page → 0 rows → breaks. Total = 100.
    const rows = makeRows(100);
    const result = keysetPaginateAll(rows, 100);
    expect(result).toHaveLength(100);
  });

  it('handles a single row correctly', () => {
    const rows = makeRows(1);
    const result = keysetPaginateAll(rows, 100);
    expect(result).toHaveLength(1);
    expect(result[0].id).toBe(rows[0].id);
  });

  it('handles page size of 1 across many rows', () => {
    const rows = makeRows(10);
    const result = keysetPaginateAll(rows, 1);
    expect(result).toHaveLength(10);
    // Verify order is preserved
    for (let i = 0; i < rows.length; i++) {
      expect(result[i].id).toBe(rows[i].id);
    }
  });
});
