// `node --test tools/site/` runs this directory as a module (Node does not expand a directory argument):
// load every *.test.mjs beside it so their tests run and report.
import { readdirSync } from 'node:fs';
const here = new URL('./', import.meta.url);
for (const f of readdirSync(here).filter(f => f.endsWith('.test.mjs')).sort()) await import(new URL(f, here));
