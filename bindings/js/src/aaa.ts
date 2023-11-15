import { OID } from "./oid";
import { EvaErrorCode, EvaError } from "./types";
import { assert, assertEq } from "./tools";

/** ACI (API call info) data payload */
export interface ACIData {
  acl: string;
  auth: string;
  token_mode: string;
  u: string;
}

/** ACI class */
export class ACI {
  aclName: string;
  auth: string;
  tokenMode: string;
  user: string;

  /**
   * @param {ACIData} aciData - construct an instance from the ACI payload
   */
  constructor(aciData: ACIData) {
    this.aclName = aciData.acl;
    this.auth = aciData.auth;
    this.tokenMode = aciData.token_mode;
    this.user = aciData.u;
  }

  /**
   * Is current sesssion writable
   *
   * @returns {boolean}
   */
  isWritable(): boolean {
    return this.auth != "token" || this.tokenMode != "readonly";
  }

  /**
   * Require current sesssion writable
   *
   * @throws {EvaError}
   * @returns {void}
   */
  requireWritable(): void {
    if (!this.isWritable()) {
      throw new EvaError(EvaErrorCode.AccessDenied, "the session is read-only");
    }
  }
}

/** ACL allow/deny block */
export interface ACLAllowDeny {
  items: Array<string>;
  pvt: Array<string>;
  rpvt: Array<string>;
}

/** ACL data payload */
export interface ACLData {
  admin?: boolean;
  deny_read: ACLAllowDeny;
  deny_write: ACLAllowDeny;
  from: Array<string>;
  id: string;
  ops: Array<string>;
  read: ACLAllowDeny;
  write: ACLAllowDeny;
  meta?: any;
}

/** ACL class */
export class ACL {
  admin?: boolean;
  denyRead: ACLAllowDeny;
  denyWrite: ACLAllowDeny;
  from: Array<string>;
  id: string;
  ops: Array<string>;
  read: ACLAllowDeny;
  write: ACLAllowDeny;
  meta?: any;

  /**
   * @param {ACLData} aclData - construct an instance from the ACL payload
   */
  constructor(aclData: ACLData) {
    this.admin = aclData.admin;
    this.denyRead = aclData.deny_read;
    this.denyWrite = aclData.deny_write;
    this.from = aclData.from;
    this.id = aclData.id;
    this.ops = aclData.ops;
    this.read = aclData.read;
    this.write = aclData.write;
    this.meta = aclData.meta;
  }

  /**
   * Constructs allow/deny OID lists (strings), useful for item.state core call
   */
  getItemsAllowDenyReading(): [Array<string>, Array<string>] {
    const allow: Set<string> = new Set();
    const deny: Set<string> = new Set();
    if (this.admin) {
      allow.add("#");
    } else {
      this.read?.items?.forEach((oid) => allow.add(oid));
      this.write?.items?.forEach((oid) => allow.add(oid));
      this.denyRead?.items?.forEach((oid) => deny.add(oid));
    }
    return [Array.from(allow), Array.from(deny)];
  }

  /**
   * Is admin
   *
   * @returns {boolean}
   */
  isAdmin(): boolean {
    return this.admin === true;
  }

  /**
   * Is an operation allowed
   *
   * @param {string} op - operation to check
   *
   * @returns {boolean}
   */
  checkOp(op: string): boolean {
    return this.isAdmin() || this.ops?.includes(op);
  }

  /**
   * Is an item readable
   *
   * @param {OID} oid - item to check
   *
   * @returns {boolean}
   */
  isItemReadable(oid: OID): boolean {
    return (
      this.isAdmin() ||
      ((oidMatch(oid, this.read.items) || oidMatch(oid, this.write.items)) &&
        !oidMatch(oid, this.denyRead.items))
    );
  }

  /**
   * Is an item writable
   *
   * @param {OID} oid - item to check
   *
   * @returns {boolean}
   */
  isItemWritable(oid: OID): boolean {
    return (
      this.isAdmin() ||
      (oidMatch(oid, this.write.items) && !oidMatch(oid, this.denyWrite.items))
    );
  }

  /**
   * Is a pvt path readable
   *
   * @param {string} path - pvt path to check
   *
   * @returns {boolean}
   */
  isPvtReadable(path: string): boolean {
    return (
      this.isAdmin() ||
      ((pathMatch(path, this.read.pvt) || pathMatch(path, this.write.pvt)) &&
        !pathMatch(path, this.denyRead.pvt))
    );
  }

  /**
   * Is a pvt path writable
   *
   * @param {string} path - pvt path to check
   *
   * @returns {boolean}
   */
  isPvtWritable(path: string): boolean {
    return (
      this.isAdmin() ||
      (pathMatch(path, this.write.items) &&
        !pathMatch(path, this.denyWrite.pvt))
    );
  }

  /**
   * Require admin
   *
   * @throws {EvaError}
   * @returns {void}
   */
  requireAdmin(): void {
    if (!this.admin) {
      throw new EvaError(EvaErrorCode.AccessDenied, "admin access required");
    }
  }

  /**
   * Require an operation
   *
   * @param {string} op - operation to check
   *
   * @throws {EvaError}
   * @returns {void}
   */
  requireOp(op: string): void {
    if (!this.checkOp(op)) {
      throw new EvaError(
        EvaErrorCode.AccessDenied,
        `operation access required: ${op}`
      );
    }
  }

  /**
   * Require an item to be readable
   *
   * @param {OID} oid - item to check
   *
   * @throws {EvaError}
   * @returns {void}
   */
  requireItemRead(oid: OID): void {
    if (!this.isItemReadable(oid)) {
      throw new EvaError(
        EvaErrorCode.AccessDenied,
        "read access required for: " + oid.asString()
      );
    }
  }

  /**
   * Require an item to be writable
   *
   * @param {OID} oid - item to check
   *
   * @throws {EvaError}
   * @returns {void}
   */
  requireItemWrite(oid: OID): void {
    if (!this.isItemWritable(oid)) {
      throw new EvaError(
        EvaErrorCode.AccessDenied,
        "write access required for: " + oid.asString()
      );
    }
  }

  /**
   * Require a pvt path to be readable
   *
   * @param {string} path - pvt path to check
   *
   * @throws {EvaError}
   * @returns {void}
   */
  requirePvtRead(path: string): void {
    if (!this.isPvtReadable(path)) {
      throw new EvaError(
        EvaErrorCode.AccessDenied,
        `read access required for: ${path}`
      );
    }
  }

  /**
   * Require a pvt path to be writable
   *
   * @param {string} path - pvt path to check
   *
   * @throws {EvaError}
   * @returns {void}
   */
  requirePvtWrite(path: string): void {
    if (!this.isPvtWritable(path)) {
      throw new EvaError(
        EvaErrorCode.AccessDenied,
        `write access required for: ${path}`
      );
    }
  }
}

/** HTTP x-call data payload */
export interface XCallData {
  method: string;
  params?: any;
  aci: ACIData;
  acl: ACLData;
}

/** HTTP x-call class */
export class XCall {
  method: string;
  params?: any;
  aci: ACI;
  acl: ACL;

  /**
   * @param {XCallData} payload - construct an instance from the x-call payload
   */
  constructor(payload: XCallData) {
    this.method = payload.method;
    this.params = payload.params;
    this.aci = new ACI(payload.aci);
    this.acl = new ACL(payload.acl);
  }
}

/**
 * Checks is OID matching given masks
 *
 * @param {OID} oid - OID to check
 * @param {Array<string>} masks - masks to check with
 *
 * @returns {boolean}
 */
export const oidMatch = (oid: OID, masks: Array<string>): boolean => {
  return pathMatch(
    oid.asPath(),
    masks.map((m) => (m === "#" ? m : new OID(m).asPath()))
  );
};

/**
 * Checks is a path matching given masks
 *
 * @param {string} path - path to check
 * @param {Array<string>} masks - masks to check with
 *
 * @returns {boolean}
 */
export const pathMatch = (path: string, masks: Array<string>): boolean => {
  if (masks.includes("#") || masks.includes(path)) {
    return true;
  }
  for (const mask of masks) {
    const g1 = mask.split("/");
    const g2 = path.split("/");
    let match = true;
    for (let i = 0; i < g1.length; i++) {
      if (i >= g2.length) {
        match = false;
        break;
      }
      if (g1[i] === "#") {
        return true;
      }
      if (g1[i] !== "+" && g1[i] !== g2[i]) {
        match = false;
        break;
      }
      if (i == g1.length - 1 && g2.length > g1.length) {
        match = false;
      }
    }
    if (match) {
      return true;
    }
  }
  return false;
};

/** @ignore */
export const selfTest = () => {
  assert(oidMatch(new OID("sensor:content/data"), ["sensor:content/#"]));
  assert(!oidMatch(new OID("sensor:content/data"), ["sensor:+"]));
  assert(oidMatch(new OID("sensor:content/data"), ["sensor:content/+"]));
  assert(oidMatch(new OID("sensor:content/data"), ["sensor:+/data"]));
  assert(oidMatch(new OID("sensor:content/data"), ["sensor:+/#"]));
  assert(oidMatch(new OID("sensor:content/data"), ["sensor:#"]));

  assert(pathMatch("content/data", ["#", "content"]));
  assert(!pathMatch("content/data", ["content"]));
  assert(pathMatch("content/data", ["content/+"]));
  assert(pathMatch("content/data", ["+/data"]));
  assert(!pathMatch("content/data", ["content/+/data"]));
  assert(pathMatch("content/data", ["content/data", "content/+/data"]));

  const payload: XCallData = {
    method: "list",
    params: {
      i: "test"
    },
    aci: {
      auth: "token",
      token_mode: "normal",
      u: "admin",
      acl: "admin"
    },
    acl: {
      id: "admin",
      read: {
        items: ["unit:#"],
        pvt: ["data/#"],
        rpvt: []
      },
      write: {
        items: [],
        pvt: [],
        rpvt: []
      },
      deny_read: {
        items: [],
        pvt: ["data/secret"],
        rpvt: []
      },
      deny_write: {
        items: [],
        pvt: ["data/secret"],
        rpvt: []
      },
      ops: ["supervisor"],
      meta: {
        admin: ["any"]
      },
      from: ["admin"]
    }
  };
  const xcall = new XCall(payload);
  assertEq(xcall.method, "list");
  assertEq(xcall.params.i, "test");
  assertEq(xcall.aci.auth, "token");
  assertEq(xcall.aci.user, "admin");
  assert(xcall.aci.isWritable());
  assert(xcall.acl.isItemReadable(new OID("unit:tests/t1")));
  assert(!xcall.acl.isItemReadable(new OID("sensor:tests/t1")));
  assert(xcall.acl.isPvtReadable("data/var1"));
  assert(!xcall.acl.isPvtReadable("data2/var1"));
  assert(!xcall.acl.isPvtReadable("data/secret"));
  assert(xcall.acl.checkOp("supervisor"));
  assert(!xcall.acl.checkOp("devices"));
};
