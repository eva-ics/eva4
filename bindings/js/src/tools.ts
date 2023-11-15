const msgpack = require("msgpackr");
const { exec } = require("node:child_process");
const { hrtime } = require("node:process");
const fs = require("fs");

export interface userIds {
  uid: number;
  gid: number;
  groupIds: Array<number>;
}

export const getUserIds = (login: string): userIds => {
  let uid;
  let gid;
  for (const line of fs.readFileSync("/etc/passwd").toString().split("\n")) {
    if (line) {
      const data = line.split(":");
      if (data[0] === login) {
        uid = parseInt(data[2]);
        gid = parseInt(data[3]);
      }
    }
  }
  if (uid === undefined || gid === undefined || isNaN(uid) || isNaN(gid)) {
    throw new Error(`unable to get user info for ${login}`);
  }
  let groups = [gid];
  for (const line of fs.readFileSync("/etc/group").toString().split("\n")) {
    if (line) {
      const data = line.split(":");
      const groupId = data[2];
      if (data[3]) {
        for (const u of data[3].split(",")) {
          if (u === login) {
            const i = parseInt(groupId);
            if (!isNaN(i)) {
              groups.push(i);
            }
          }
          break;
        }
      }
    }
  }
  return {
    uid: uid,
    gid: gid,
    groupIds: groups
  };
};

export const shellCommand = (command: string): Promise<void> => {
  return new Promise((resolve, reject) => {
    exec(command, (error, stdout, stderr) => {
      if (stdout) {
        console.log(stdout);
      }
      if (stderr) {
        console.error(stderr);
      }
      if (error) {
        reject(`prepare command failed with the error: ${error}`);
      } else {
        resolve();
      }
    });
  });
};

export const readStdin = (size: number): Promise<Buffer> => {
  return new Promise((resolve, reject) => {
    const onReadable = () => {
      process.stdin.off("readable", onReadable);
      resolve(Buffer.from(process.stdin.read(size)));
    };
    const onError = (err: any) => {
      process.stdin.off("error", onReadable);
      reject(err);
    };
    process.stdin.on("readable", onReadable);
    process.stdin.on("error", onError);
  });
};

/** Get monotonic clock as a float number */
export const clockMonotonic = (): number => {
  return parseInt(hrtime.bigint()) / 1_000_000_000;
};

/** Packs a payload to send via EAPI (MessagePack) */
export const pack = msgpack.pack;
/**
* Unpacks a payload, received via EAPI (MessagePack), automatically returns
* undefined if the payload is empty
*
* @param {Buffer|Array<number>} payload - payload to unpack
*
* @returns {any}
*/
export const unpack = (payload: Buffer | Array<number>): any => {
  return payload.length > 0 ? msgpack.unpack(payload) : undefined;
};

export const assert = (cond: any): void => {
  if (!cond) {
    throw new Error();
  }
};

export const assertEq = (first: any, second: any): void => {
  if (first !== second) {
    throw new Error(`${first} !== ${second}`);
  }
};

export const assertNe = (first: any, second: any): void => {
  if (first === second) {
    throw new Error(`${first} === ${second}`);
  }
};
