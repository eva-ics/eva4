import { ActionStatusCode, EapiTopic } from "./types";
import { OID } from "./oid";
import { Bus, QoS } from "busrt";
import { pack } from "./tools";
import { stringify as uuidStringify, parse as uuidParse } from "uuid";

/** EAPI action status payload */
export interface BusActionStatus {
  uuid: Array<number> | Uint8Array;
  status: ActionStatusCode;
  out?: any;
  err?: any;
  exitcode?: number;
}

/** EAPI action call payload */
export interface BusAction {
  uuid: Array<number> | Buffer;
  i: string;
  timeout: number; // microseconds
  priority: number;
  params?: ActionParams;
  config?: any;
}

/** EAPI action call params payload */
export interface ActionParams {
  /** for unit actions */
  value?: any;
  /** for lmacro actions */
  args?: Array<any>;
  /** for lmacro actions */
  kwargs?: object;
}

/** EAPI action call class */
export class Action {
  uuid: string;
  oid: OID;
  timeout: number;
  priority: number;
  params?: ActionParams;
  config?: any;

  /**
   * @param {event} BusAction - construct an instance from the bus call payload
   */
  constructor(event: BusAction) {
    this.uuid = uuidStringify(event.uuid);
    this.oid = new OID(event.i);
    this.timeout = event.timeout;
    this.priority = event.priority;
    this.params = event.params;
    this.config = event.config;
  }
}

/** Action controller */
export class Controller {
  /** @ignore */
  bus: Bus;

  /** @ignore */
  constructor(bus: Bus) {
    this.bus = bus;
  }

  /**
   * Notify the bus about action pending event
   *
   * @param {Action} action - action to notify about
   *
   * @returns {Promise<void>}
   */
  eventPending(action: Action): Promise<void> {
    return this._sendEvent(action, ActionStatusCode.Pending);
  }

  /**
   * Notify the bus about action running event
   *
   * @param {Action} action - action to notify about
   *
   * @returns {Promise<void>}
   */
  eventRunning(action: Action): Promise<void> {
    return this._sendEvent(action, ActionStatusCode.Running);
  }

  /**
   * Notify the bus about action completed event
   *
   * @param {Action} action - action to notify about
   * @param {any} [out] - output result
   *
   * @returns {Promise<void>}
   */
  eventCompleted(action: Action, out?: any): Promise<void> {
    return this._sendEvent(
      action,
      ActionStatusCode.Completed,
      out,
      undefined,
      0
    );
  }

  /**
   * Notify the bus about action failed event
   *
   * @param {Action} action - action to notify about
   * @param {any} [out] - output result
   * @param {any} [err] - error message
   * @param {number} [exitcode] - exit code (default: -1)
   *
   * @returns {Promise<void>}
   */
  eventFailed(
    action: Action,
    out?: any,
    err?: any,
    exitcode?: number
  ): Promise<void> {
    return this._sendEvent(
      action,
      ActionStatusCode.Failed,
      out,
      err,
      exitcode === undefined ? -1 : exitcode
    );
  }

  /**
   * Notify the bus about action canceled event
   *
   * @param {Action} action - action to notify about
   *
   * @returns {Promise<void>}
   */
  eventCanceled(action: Action) {
    return this._sendEvent(action, ActionStatusCode.Canceled);
  }

  /**
   * Notify the bus about action terminated event
   *
   * @param {Action} action - action to notify about
   *
   * @returns {Promise<void>}
   */
  eventTerminated(action: Action) {
    return this._sendEvent(action, ActionStatusCode.Terminated);
  }

  /** @ignore */
  async _sendEvent(
    action: Action,
    statusCode: ActionStatusCode,
    out?: any,
    err?: any,
    exitcode?: number
  ): Promise<void> {
    const path = action.oid.asPath();
    const payload = this._actionEventPayload(
      action.uuid,
      statusCode,
      out,
      err,
      exitcode
    );
    this.bus.publish(`${EapiTopic.ActionStatus}${path}`, payload, QoS.No);
  }

  /** @ignore */
  _actionEventPayload(
    u: string,
    status: ActionStatusCode,
    out?: any,
    err?: any,
    exitcode?: number
  ): Buffer {
    let payload: BusActionStatus = {
      uuid: uuidParse(u),
      status: status
    };
    if (out !== undefined) {
      payload.out = out;
    }
    if (err !== undefined) {
      payload.err = err;
    }
    if (exitcode !== undefined) {
      payload.exitcode = exitcode;
    }
    return pack(payload);
  }
}
