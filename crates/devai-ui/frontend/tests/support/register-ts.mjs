// Registers the `.ts` strip loader (see ts-loader.mjs) before the test
// runner spawns/executes the test files: `node --import ./tests/support/register-ts.mjs`.
import { register } from "node:module";

register("./ts-loader.mjs", import.meta.url);
