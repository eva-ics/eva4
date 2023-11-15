import * as path from "path";
import { defineConfig } from "vite";

const lib_name = "sdk";

export default defineConfig({
  build: {
    rollupOptions: {
      external: [
        "busrt",
        "get-stdin",
        "msgpackr",
        "sleep-promise",
        "uuid",
        "node:fs",
        "node:process",
        "node:child_process"
      ]
    },
    lib: {
      entry: path.resolve(__dirname, "src/lib.ts"),
      name: lib_name,
      fileName: (format) => `${lib_name}.${format}.js`
    }
  }
});
