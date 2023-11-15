/** Item kind */
export enum ItemKind {
  Unit = "unit",
  Sensor = "sensor",
  LVar = "lvar",
  LMacro = "lmacro",
  Any = "+"
}

/** Item OID */
export class OID {
  kind: ItemKind;
  full_id: string;

  /**
   * @param {string} s - construct OID from a string
   * @param {boolean} [from_path] - string is a path instead of the regular OID
   */
  constructor(s: string, from_path?: boolean) {
    const pos = s.indexOf(from_path ? "/" : ":");
    if (pos == -1) {
      throw new Error(`invalid OID: ${s}`);
    }
    let kind;
    [kind, this.full_id] = [s.slice(0, pos), s.slice(pos + 1)];
    if (!Object.values(ItemKind).includes(kind as ItemKind)) {
      throw new Error(`invalid item kind: ${kind}`);
    }
    this.kind = kind as ItemKind;
  }

  /**
   * Convert OID to a string, use every time when OID needs to be converted
   * into a string instead of JS defaults
   *
   * @returns {string}
   */
  asString(): string {
    return `${this.kind}:${this.full_id}`;
  }

  /**
   * Convert OID to a path
   *
   * @returns {string}
   */
  asPath(): string {
    return `${this.kind}/${this.full_id}`;
  }

  /**
   * Compare OID with another one
   *
   * @param {OID} other - OID to compare with
   *
   * @returns {boolean}
   */
  equal(other: OID): boolean {
    return other.kind === this.kind && other.full_id === this.full_id;
  }
}
