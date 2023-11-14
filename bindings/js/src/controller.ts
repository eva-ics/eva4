import { ActionStatusCode, EapiTopic } from "./types";
import { OID } from "./oid";
import { Bus, QoS } from "busrt";
import { pack } from "./tools";
import { stringify as uuidStringify, parse as uuidParse } from "uuid";

export interface BusActionStatus {
  uuid: Array<number>;
  status: ActionStatusCode;
  out?: any;
  err?: any;
  exitcode?: number;
}

export interface BusAction {
  uuid: Array<number> | Buffer;
  i: string;
  timeout: number; // microseconds
  priority: number;
  params?: ActionParams;
  config?: any;
}

export interface ActionParams {
  value?: any;
  args?: Array<any>;
  kwargs?: object;
}

export class Action {
  uuid: string;
  oid: OID;
  timeout: number;
  priority: number;
  params?: ActionParams;
  config?: any;

  constructor(event: BusAction) {
    this.uuid = uuidStringify(event.uuid);
    this.oid = new OID(event.i);
    this.timeout = event.timeout;
    this.priority = event.priority;
    this.params = event.params;
    this.config = event.config;
  }
}

export class Controller {
  bus: Bus;

  constructor(bus: Bus) {
    this.bus = bus;
  }

  eventPending(action: Action) {
    this._sendEvent(action, ActionStatusCode.Pending);
  }

  eventRunning(action: Action) {
    this._sendEvent(action, ActionStatusCode.Running);
  }

  eventCompleted(action: Action, out?: any) {
    this._sendEvent(action, ActionStatusCode.Completed, out);
  }

  eventFailed(action: Action, out?: any, err?: any, exitcode?: number) {
    this._sendEvent(action, ActionStatusCode.Failed, out, err, exitcode);
  }

  eventCanceled(action: Action) {
    this._sendEvent(action, ActionStatusCode.Canceled);
  }

  eventTerminated(action: Action) {
    this._sendEvent(action, ActionStatusCode.Terminated);
  }

  _sendEvent(
    action: Action,
    statusCode: ActionStatusCode,
    out?: any,
    err?: any,
    exitcode?: number
  ) {
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
