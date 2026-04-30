import { inspect } from "node:util";

export type LogLevel = "debug" | "info" | "warn" | "error";
export type LogFields = Record<string, unknown>;

const LEVEL_ORDER: Record<LogLevel, number> = {
    debug: 10,
    info: 20,
    warn: 30,
    error: 40,
};

const LEVEL_LABEL: Record<LogLevel, string> = {
    debug: "DEBUG",
    info: " INFO",
    warn: " WARN",
    error: "ERROR",
};

const SERVICE = "bears-codepool";
const LOG_STYLE = (process.env.LOG_STYLE ?? "pretty").trim().toLowerCase();
const MIN_LEVEL = parseLogLevel(process.env.LOG_LEVEL, "info");

function parseLogLevel(value: string | undefined, fallback: LogLevel): LogLevel {
    const normalized = value?.trim().toLowerCase();
    if (
        normalized === "debug" ||
        normalized === "info" ||
        normalized === "warn" ||
        normalized === "error"
    ) {
        return normalized;
    }
    return fallback;
}

function shouldLog(level: LogLevel): boolean {
    return LEVEL_ORDER[level] >= LEVEL_ORDER[MIN_LEVEL];
}

function toErrorFields(error: unknown): LogFields {
    if (error instanceof Error) {
        return {
            error: error.message,
            error_name: error.name,
            stack: error.stack,
        };
    }
    return { error: String(error) };
}

function normalizeFields(fields?: LogFields): LogFields {
    if (!fields) return {};
    const out: LogFields = {};
    for (const [key, value] of Object.entries(fields)) {
        if (value instanceof Error) {
            out[key] = value.message;
            out[`${key}_name`] = value.name;
            out[`${key}_stack`] = value.stack;
        } else if (value !== undefined) {
            out[key] = value;
        }
    }
    return out;
}

function formatValue(value: unknown): string {
    if (value === null) return "null";
    if (typeof value === "string") {
        return value.includes(" ") || value.includes("=")
            ? JSON.stringify(value)
            : value;
    }
    if (
        typeof value === "number" ||
        typeof value === "boolean" ||
        typeof value === "bigint"
    ) {
        return String(value);
    }
    return inspect(value, {
        colors: false,
        depth: 4,
        breakLength: 120,
        compact: true,
        sorted: true,
    });
}

function formatPrettyLine(level: LogLevel, message: string, fields: LogFields): string {
    const timestamp = new Date().toISOString();
    const event = typeof fields.event === "string" ? fields.event : undefined;
    const details = Object.entries(fields)
        .filter(([key]) => key !== "event" && key !== "service")
        .map(([key, value]) => `${key}=${formatValue(value)}`)
        .join(" ");
    const eventPart = event ? ` ${event}:` : "";
    return `${timestamp} ${LEVEL_LABEL[level]} ${SERVICE}${eventPart} ${message}${details ? ` ${details}` : ""}`;
}

function write(level: LogLevel, message: string, fields?: LogFields): void {
    if (!shouldLog(level)) return;

    const normalizedFields = {
        service: SERVICE,
        ...normalizeFields(fields),
    };

    const line =
        LOG_STYLE === "json"
            ? JSON.stringify({
                  timestamp: new Date().toISOString(),
                  level,
                  message,
                  ...normalizedFields,
              })
            : formatPrettyLine(level, message, normalizedFields);

    if (level === "error") {
        console.error(line);
    } else if (level === "warn") {
        console.warn(line);
    } else {
        console.log(line);
    }
}

export const logger = {
    debug(message: string, fields?: LogFields): void {
        write("debug", message, fields);
    },
    info(message: string, fields?: LogFields): void {
        write("info", message, fields);
    },
    warn(message: string, fields?: LogFields): void {
        write("warn", message, fields);
    },
    error(message: string, fields?: LogFields | unknown): void {
        if (
            fields &&
            typeof fields === "object" &&
            !Array.isArray(fields) &&
            !(fields instanceof Error)
        ) {
            write("error", message, fields as LogFields);
        } else if (fields !== undefined) {
            write("error", message, toErrorFields(fields));
        } else {
            write("error", message);
        }
    },
};
