import { Bus } from "busrt";
import { LogLevelCode, LogLevelName, EapiTopic } from "./types";

/**
 * Bus logger, recommened to use instead of console.log
 *
 * Constructed automatically in service instances
 */
export class Logger {
  /** @ignore */
  bus: Bus;
  /** @ignore */
  minLevel: number;

  /** @ignore */
  constructor(bus: Bus, minLevel: number) {
    this.bus = bus;
    this.minLevel = minLevel;
  }

  /**
   * Send a "trace" level message
   */
  async trace(...msgs: any[]): Promise<void> {
    if (this.minLevel <= LogLevelCode.Trace) {
      await this._log(LogLevelName.Trace, msgs);
    }
  }

  /**
   * Send a "debug" level message
   */
  async debug(...msgs: any[]): Promise<void> {
    if (this.minLevel <= LogLevelCode.Debug) {
      await this._log(LogLevelName.Debug, msgs);
    }
  }

  /**
   * Send an "info" level message
   */
  async info(...msgs: any[]): Promise<void> {
    if (this.minLevel <= LogLevelCode.Info) {
      await this._log(LogLevelName.Info, msgs);
    }
  }

  /**
   * Send a "warn" level message
   */
  async warn(...msgs: any[]): Promise<void> {
    if (this.minLevel <= LogLevelCode.Warn) {
      await this._log(LogLevelName.Warn, msgs);
    }
  }

  /**
   * Send an "error" level message
   */
  async error(...msgs: any[]): Promise<void> {
    if (this.minLevel <= LogLevelCode.Error) {
      await this._log(LogLevelName.Error, msgs);
    }
  }

  /** @ignore */
  async _log(level: LogLevelName, msgs: any[]): Promise<void> {
    const prepared: Array<string> = msgs.map((s) => {
      return typeof s === "object" ? JSON.stringify(s) : s.toString();
    });
    await this.bus.publish(
      `${EapiTopic.LogInputTopic}${level}`,
      Buffer.from(prepared.join(" "))
    );
  }
}
