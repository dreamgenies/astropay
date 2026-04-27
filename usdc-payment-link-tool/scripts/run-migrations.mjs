import fs from 'node:fs/promises';
import path from 'node:path';
import pg from 'pg';

const { Client } = pg;

async function loadLocalEnvFile(filename) {
  const filePath = path.join(process.cwd(), filename);
  try {
    const raw = await fs.readFile(filePath, 'utf8');
    for (const line of raw.split(/\r?\n/)) {
      const trimmed = line.trim();
      if (!trimmed || trimmed.startsWith('#')) continue;
      const separator = trimmed.indexOf('=');
      if (separator === -1) continue;
      const key = trimmed.slice(0, separator).trim();
      if (!key || process.env[key] !== undefined) continue;
      let value = trimmed.slice(separator + 1).trim();
      if (
        (value.startsWith('"') && value.endsWith('"')) ||
        (value.startsWith("'") && value.endsWith("'"))
      ) {
        value = value.slice(1, -1);
      }
      process.env[key] = value;
    }
  } catch (error) {
    if (error && typeof error === 'object' && 'code' in error && error.code === 'ENOENT') return;
    throw error;
  }
}

await loadLocalEnvFile('.env.local');
await loadLocalEnvFile('.env');

const connectionString = process.env.DATABASE_URL;
if (!connectionString) {
  throw new Error('DATABASE_URL is required to run migrations');
}

const migrationsDir = path.join(process.cwd(), 'migrations');
const files = (await fs.readdir(migrationsDir)).filter((name) => name.endsWith('.sql')).sort();
const client = new Client({ connectionString, ssl: process.env.PGSSL === 'disable' ? false : { rejectUnauthorized: false } });
await client.connect();

await client.query(`
  CREATE TABLE IF NOT EXISTS schema_migrations (
    id         TEXT PRIMARY KEY,
    applied_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    applied_by TEXT        NOT NULL DEFAULT 'unknown'
  )
`);
await client.query(`
  ALTER TABLE schema_migrations ADD COLUMN IF NOT EXISTS applied_by TEXT NOT NULL DEFAULT 'unknown'
`);

for (const file of files) {
  const exists = await client.query('SELECT 1 FROM schema_migrations WHERE id = $1', [file]);
  if (exists.rowCount) continue;
  const sql = await fs.readFile(path.join(migrationsDir, file), 'utf8');
  await client.query('BEGIN');
  try {
    await client.query(sql);
    await client.query("INSERT INTO schema_migrations (id, applied_by) VALUES ($1, 'nextjs')", [file]);
    await client.query('COMMIT');
    console.log(`Applied ${file}`);
  } catch (error) {
    await client.query('ROLLBACK');
    throw error;
  }
}

await client.end();
