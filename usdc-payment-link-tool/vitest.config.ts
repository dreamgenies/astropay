import path from 'node:path';
import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    environment: 'node',
    include: ['**/*.test.ts'],
    testTimeout: 10_000,
  },
  resolve: {
    alias: { '@': path.resolve(__dirname, '.') },
  },
});
