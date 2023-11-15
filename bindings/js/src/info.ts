/** Service info method **/
export class ServiceMethod {
  /** @ignore */
  name: string;
  /** @ignore */
  description: string;
  /** @ignore */
  requiredParams: Array<string>;
  /** @ignore */
  optionalParams: Array<string>;

  /**
   * @param {string} name - method name
   * @param {string} [description] - method description
   */
  constructor(name: string, description?: string) {
    this.name = name;
    this.description = description || "";
    this.requiredParams = [];
    this.optionalParams = [];
  }

  /**
   * Set a required parameter (chained)
   *
   * @param {string} name - param name
   *
   * @returns {ServiceMethod}
   */
  required(name: string): ServiceMethod {
    this.requiredParams.push(name);
    return this;
  }

  /**
   * Set an optional parameter (chained)
   *
   * @param {string} name - param name
   *
   * @returns {ServiceMethod}
   */
  optional(name: string): ServiceMethod {
    this.optionalParams.push(name);
    return this;
  }
}

/** Service info **/
export class ServiceInfo {
  author: string;
  /** @ignore */
  description: string;
  /** @ignore */
  version: string;
  /** @ignore */
  methods: object;

  /**
   * @param {string} params.author - service author
   * @param {string} params.description - service description (long name)
   * @param {version} params.version - service version
   */
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

  /**
   * Add a RPC method (chained)
   *
   * @param {ServiceMethod} method - method to add
   *
   * @returns {ServiceInfo}
   */
  addMethod(method: ServiceMethod): ServiceInfo {
    const params = {};
    for (const p of method.requiredParams) {
      (params as any)[p] = { required: true };
    }
    for (const p of method.optionalParams) {
      (params as any)[p] = { required: false };
    }
    (this.methods as any)[method.name] = {
      description: method.description,
      params: params
    };
    return this;
  }
}
