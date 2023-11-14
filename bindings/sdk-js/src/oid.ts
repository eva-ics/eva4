export enum ItemKind {
  Unit = "unit",
  Sensor = "sensor",
  LVar = "lvar",
  LMacro = "lmacro",
  Any = "+"
}

export class OID {
  kind: ItemKind;
  full_id: string;

  constructor(s: string, from_path?: boolean) {
    const pos = s.indexOf(from_path ? "/" : ":");
    if (pos == -1) {
      throw new Error(`invalid OID: ${s}`);
    }
    let kind;
    [kind, this.full_id] = [s.slice(0, pos), s.slice(pos + 1)];
    if (!Object.values(ItemKind).includes(kind)) {
      throw new Error(`invalid item kind: ${kind}`);
    }
    this.kind = kind;
  }

  asString(): string {
    return `${this.kind}:${this.full_id}`;
  }

  asPath(): string {
    return `${this.kind}/${this.full_id}`;
  }

  equal(other: OID): boolean {
    return other.kind === this.kind && other.full_id === this.full_id;
  }
}
