import { Bus } from "busrt";
import { LogLevelCode, LogLevelName, EapiTopic } from "./types";

export class Logger {
  bus: Bus;
  minLevel: number;

  constructor(bus: Bus, minLevel: number) {
    this.bus = bus;
    this.minLevel = minLevel;
  }

  async trace(...msgs: any[]): Promise<void> {
    if (this.minLevel <= LogLevelCode.Trace) {
      await this._log(LogLevelName.Trace, msgs);
    }
  }

  async debug(...msgs: any[]): Promise<void> {
    if (this.minLevel <= LogLevelCode.Debug) {
      await this._log(LogLevelName.Debug, msgs);
    }
  }

  async info(...msgs: any[]): Promise<void> {
    if (this.minLevel <= LogLevelCode.Info) {
      await this._log(LogLevelName.Info, msgs);
    }
  }

  async warn(...msgs: any[]): Promise<void> {
    if (this.minLevel <= LogLevelCode.Warn) {
      await this._log(LogLevelName.Warn, msgs);
    }
  }

  async error(...msgs: any[]): Promise<void> {
    if (this.minLevel <= LogLevelCode.Error) {
      await this._log(LogLevelName.Error, msgs);
    }
  }

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
