#!/usr/bin/env node

const fs = require("fs");

const argv = require("minimist")(process.argv.slice(2));
const silent = argv.s || false;
const command = argv._[0];

const log = (...args) => {
  if (!silent) {
    console.log(...args);
  }
};

const usage = () => {
  console.log("Usage: eva-cli [options] <command> [arg=value]...");
  console.log("Options:");
  console.log("  --config <file>  Configuration file (default: config.json)");
  console.log("  -s               Silent mode");
  console.log("  -h, --help       Display this help");
  console.log("Commands:");
  console.log("  See https://info.bma.ai/en/actual/eva4/svc/eva-hmi.html#http-api");
  console.log("Getting argument value from a file:");
  console.log("  Specify argument as arg=@file");
};

if (!command) {
  console.error("Command not specified");
  usage();
  process.exit(1);
}

const pos = command.indexOf("=");
if (pos > 0) {
  console.error("Command not specified");
  usage();
  process.exit(1);
}

const parameters = {};

for (const arg of argv._.slice(1)) {
  const pos = arg.indexOf("=");
  if (!pos) {
    console.error("Invalid argument: " + arg);
    usage();
    process.exit(1);
  }
  const key = arg.slice(0, pos);
  let value = arg.slice(pos + 1);
  if (value.startsWith("@")) {
    log(`Reading file ${value.slice(1)}...`);
    value = fs.readFileSync(value.slice(1), "utf8");
  } else if (value === "null") {
    value = null;
  } else if (value === "true") {
    value = true;
  } else if (value === "false") {
    value = false;
  } else {
    const num = parseInt(value);
    if (!isNaN(num)) {
      value = num;
    }
    const numf = parseFloat(value);
    if (!isNaN(numf)) {
      value = numf;
    }
  }
  parameters[key] = value;
}

if (!command) {
  console.error("No command specified");
  usage();
  process.exit(1);
}

if (argv.help || argv.h) {
  usage();
  process.exit(0);
}

const config_file = argv.config || "config.json";

const config = JSON.parse(fs.readFileSync(config_file, "utf8"));

const Eva = require("@eva-ics/webengine").Eva;

const eva = new Eva();
eva.apply_config(config);

let command_processed = false;
eva.ws_mode = false;
eva.state_updates = false;

const error_str = (e) => {
  return `${e.message} (${e.code})`;
};

const process_command = async (argv) => {
  try {
    const result = await eva.call(command, parameters);
    console.log(JSON.stringify(result, null, 2));
  } catch (e) {
    console.error(`API call failed: ${error_str(e)}`);
    process.exit(1);
  }
  command_processed = true;
};

log(`Connecting to ${config.engine.api_uri}...`);

if (config.engine.login) {
  log("Logging in...");
  eva.on("login.success", () => {
    log("Authenticated successfully");
    process_command(argv);
  });
  eva.on("login.failed", (e) => {
    console.error(`Login failed: ${error_str(e)}`);
    process.exit(1);
  });
  eva.start();
} else {
  process_command(argv);
}

setInterval(() => {
  if (command_processed) {
    log("Command processed");
    if (eva.logged_in) {
      eva.stop().then(() => {
        log("Logging out");
        process.exit(0);
      });
    } else {
      process.exit(0);
    }
  }
}, 100);
