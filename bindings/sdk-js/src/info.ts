export class ServiceMethod {
  name: string;
  description: string;
  requiredParams: Array<string>;
  optionalParams: Array<string>;

  constructor(name: string, description?: string) {
    this.name = name;
    this.description = description || "";
    this.requiredParams = [];
    this.optionalParams = [];
  }

  required(name: string): ServiceMethod {
    this.requiredParams.push(name);
    return this;
  }

  optional(name: string): ServiceMethod {
    this.optionalParams.push(name);
    return this;
  }
}

export class ServiceInfo {
  author: string;
  description: string;
  version: string;
  methods: object;

  constructor({
    author,
    description,
    version
  }: {
    author: string;
    description?: string;
    version: string;
  }) {
    this.author = author || "";
    this.description = description || "";
    this.version = version || "";
    this.methods = {};
  }

  addMethod(method: ServiceMethod): ServiceInfo {
    const params = {};
    for (const p of method.requiredParams) {
      params[p] = { required: true };
    }
    for (const p of method.optionalParams) {
      params[p] = { required: false };
    }
    this.methods[method.name] = {
      description: method.description,
      params: params
    };
    return this;
  }
}

