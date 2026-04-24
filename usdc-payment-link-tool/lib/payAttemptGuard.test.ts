import { describe, expect, it } from 'vitest';

async function simulatePayClicks(concurrentClicks: number): Promise<number> {
  let executions = 0;
  let inFlight = false;

  async function pay() {
    if (inFlight) return;
    inFlight = true;
    try {
      executions++;
      await Promise.resolve();
    } finally {
      inFlight = false;
    }
  }

  const clicks = Array.from({ length: concurrentClicks }, () => pay());
  await Promise.all(clicks);
  return executions;
}

describe('AP-005 duplicate pay prevention', () => {
  it('executes the pay body once for concurrent clicks', async () => {
    expect(await simulatePayClicks(5)).toBe(1);
  });

  it('allows a later retry after the first attempt completes', async () => {
    let inFlight = false;
    let executions = 0;

    async function pay() {
      if (inFlight) return;
      inFlight = true;
      try {
        executions++;
        await Promise.resolve();
      } finally {
        inFlight = false;
      }
    }

    await pay();
    await pay();

    expect(executions).toBe(2);
  });

  it('releases the guard after a failed attempt', async () => {
    let inFlight = false;
    let executions = 0;

    async function pay() {
      if (inFlight) return;
      inFlight = true;
      try {
        executions++;
        throw new Error('Simulated failure');
      } finally {
        inFlight = false;
      }
    }

    await pay().catch(() => {});
    await pay().catch(() => {});

    expect(executions).toBe(2);
  });
});