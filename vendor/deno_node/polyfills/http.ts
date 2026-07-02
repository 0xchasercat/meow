// Copyright 2018-2026 the Deno authors. MIT license.
// deno-lint-ignore-file no-explicit-any

(function () {
const { core, primordials } = __bootstrap;
const { ArrayPrototypeSlice, ArrayPrototypeSort } = primordials;

const {
  OutgoingMessage,
  validateHeaderName,
  validateHeaderValue,
} = core.createLazyLoader("node:_http_outgoing")();
const { ClientRequest } = core.createLazyLoader("node:_http_client")();
const httpAgent = core.createLazyLoader("node:_http_agent")();
const { Agent } = httpAgent;
const httpProxy = core.createLazyLoader("node:_http_proxy")();
const { setGlobalProxyFromEnv } = httpProxy;
const { IncomingMessage } = core.createLazyLoader("node:_http_incoming")();
const {
  _connectionListener,
  Server: ServerImpl,
  ServerResponse,
  STATUS_CODES,
} = core.createLazyLoader("node:_http_server")();
const { methods, parsers } = core.createLazyLoader("node:_http_common")();
const { validateInteger } = core.loadExtScript(
  "ext:deno_node/internal/validators.mjs",
);
const METHODS = ArrayPrototypeSort(ArrayPrototypeSlice(methods));

interface RequestOptions {
  agent?: Agent;
  auth?: string;
  createConnection?: () => unknown;
  defaultPort?: number;
  family?: number;
  headers?: Record<string, string>;
  hints?: number;
  host?: string;
  hostname?: string;
  insecureHTTPParser?: boolean;
  localAddress?: string;
  lookup?: () => void;
  maxHeaderSize?: number;
  method?: string;
  path?: string;
  port?: number;
  protocol?: string;
  setHost?: boolean;
  socketPath?: string;
  timeout?: number;
  signal?: AbortSignal;
  href?: string;
}

type ServerHandler = (req: any, res: any) => void;

const { nextTick } = core.loadExtScript("ext:deno_node/_next_tick.ts");

let kOutHeadersSymbol: any = null;
let kHeadersSymbol: any = null;
let kHeadersCountSymbol: any = null;

function getOutHeadersSymbol(res: any) {
  if (kOutHeadersSymbol !== null) return kOutHeadersSymbol;
  const symbols = Object.getOwnPropertySymbols(res);
  const found = symbols.find(s => s.description === "kOutHeaders");
  if (found) {
    kOutHeadersSymbol = found;
  }
  return kOutHeadersSymbol;
}

function getHeadersSymbols(req: any) {
  if (kHeadersSymbol !== null) return;
  const symbols = Object.getOwnPropertySymbols(req);
  const h = symbols.find(s => s.description === "kHeaders");
  const hc = symbols.find(s => s.description === "kHeadersCount");
  if (h) kHeadersSymbol = h;
  if (hc) kHeadersCountSymbol = hc;
}

class FastServerResponse extends (ServerResponse as any) {
  slot: number;
  finished: boolean;
  headersSent: boolean;
  _chunks: Uint8Array[] | undefined;

  constructor(req: any, slot: number) {
    super(req);
    this.slot = slot;
    this.finished = false;
    this.headersSent = false;
    this.statusCode = 200;
  }

  writeHead(statusCode: number, statusMessage: any, headers: any) {
    if (typeof statusMessage === "object") {
      headers = statusMessage;
      statusMessage = undefined;
    }
    this.statusCode = statusCode;
    if (headers) {
      for (const [k, v] of Object.entries(headers)) {
        this.setHeader(k, v);
      }
    }
    this.headersSent = true;
    return this;
  }

  write(chunk: any, encoding: any, callback: any) {
    if (typeof encoding === "function") {
      callback = encoding;
      encoding = null;
    }
    if (!this._chunks) this._chunks = [];
    if (typeof chunk === "string") {
      this._chunks.push(new TextEncoder().encode(chunk));
    } else if (chunk instanceof Uint8Array) {
      this._chunks.push(chunk);
    } else if (chunk) {
      this._chunks.push(new TextEncoder().encode(String(chunk)));
    }
    if (callback) nextTick(callback);
    return true;
  }

  end(chunk: any, encoding: any, callback: any) {
    if (this.finished) return this;
    this.finished = true;

    if (typeof chunk === "function") {
      callback = chunk;
      chunk = null;
      encoding = null;
    } else if (typeof encoding === "function") {
      callback = encoding;
      encoding = null;
    }

    if (chunk) {
      this.write(chunk, encoding, null);
    }

    let bodyBytes;
    if (this._chunks && this._chunks.length > 0) {
      if (this._chunks.length === 1) {
        bodyBytes = this._chunks[0];
      } else {
        let totalLen = 0;
        for (let i = 0; i < this._chunks.length; i++) {
          totalLen += this._chunks[i].length;
        }
        bodyBytes = new Uint8Array(totalLen);
        let offset = 0;
        for (let i = 0; i < this._chunks.length; i++) {
          bodyBytes.set(this._chunks[i], offset);
          offset += this._chunks[i].length;
        }
      }
    } else {
      bodyBytes = new Uint8Array(0);
    }

    const kOutHeaders = getOutHeadersSymbol(this);
    const headersList: Array<[string, string]> = [];
    if (kOutHeaders) {
      const headersMap = this[kOutHeaders];
      if (headersMap) {
        for (const key of Object.keys(headersMap)) {
          const [originalName, value] = headersMap[key];
          if (Array.isArray(value)) {
            for (let i = 0; i < value.length; i++) {
              headersList.push([originalName, String(value[i])]);
            }
          } else {
            headersList.push([originalName, String(value)]);
          }
        }
      }
    }

    core.ops.op_http_respond(this.slot, this.statusCode, headersList, bodyBytes)
      .then(() => {
        this.emit("finish");
        this.emit("close");
        if (callback) callback();
      })
      .catch((err: any) => {
        this.emit("error", err);
      });

    return this;
  }
}

class FastServer extends (ServerImpl as any) {
  _rid: number | undefined;
  _address: any;

  constructor(options: any, requestListener: any) {
    super(options, requestListener);
    this._rid = undefined;
    this._address = null;
  }

  listen(...args: any[]) {
    let port = 0;
    let hostname = "0.0.0.0";
    let cb = null;
    let isTcp = false;

    if (typeof args[0] === "object" && args[0] !== null) {
      const options = args[0];
      port = options.port;
      hostname = options.host || options.hostname || "0.0.0.0";
      isTcp = port !== undefined;
      if (typeof args[1] === "function") cb = args[1];
    } else if (typeof args[0] === "number" || (typeof args[0] === "string" && !isNaN(Number(args[0])))) {
      port = Number(args[0]);
      isTcp = true;
      if (typeof args[1] === "string") {
        hostname = args[1];
        if (typeof args[2] === "function") cb = args[2];
        else if (typeof args[3] === "function") cb = args[3];
      } else if (typeof args[1] === "function") {
        cb = args[1];
      } else if (typeof args[2] === "function") {
        cb = args[2];
      }
    }

    if (!isTcp) {
      return super.listen(...args);
    }

    // In worker processes (e.g., Next.js build workers spawned via
    // child_process.fork()), the meow HTTP ops may not be registered.
    // Fall back to the standard slow-path ServerImpl in that case.
    if (typeof core.ops.op_http_serve !== "function") {
      return super.listen(...args);
    }

    let handle;
    try {
      handle = core.ops.op_http_serve(hostname, port);
    } catch (err) {
      // op_http_serve can fail if the ops are stubbed or if the address
      // is already in use. Fall back to the standard path.
      return super.listen(...args);
    }

    this._rid = handle.rid;
    this._address = handle.addr;

    if (cb) {
      this.once("listening", cb);
    }

    const runAcceptLoop = async () => {
      const rid = this._rid;
      try {
        while (this._rid === rid) {
          let next;
          try {
            next = await core.ops.op_http_next(rid);
          } catch {
            break;
          }
          if (next === null) {
            break;
          }

          const mockSocket = {
            encrypted: false,
            remoteAddress: "127.0.0.1",
            remotePort: 12345,
            remoteFamily: "IPv4",
            writable: true,
            readable: true,
            destroy() {},
            end() {},
            write() {},
            on() {},
            once() {},
            emit() {},
          };

          const req = new IncomingMessage(mockSocket);
          
          let reqUrl = next.url;
          const protoIdx = reqUrl.indexOf("://");
          if (protoIdx !== -1) {
            const pathIdx = reqUrl.indexOf("/", protoIdx + 3);
            reqUrl = pathIdx !== -1 ? reqUrl.substring(pathIdx) : "/";
          }
          req.url = reqUrl;
          req.method = next.method;

          const rawHeaders = [];
          for (let i = 0; i < next.headers.length; i++) {
            rawHeaders.push(next.headers[i][0], next.headers[i][1]);
          }
          req.rawHeaders = rawHeaders;
          
          getHeadersSymbols(req);
          if (kHeadersCountSymbol) {
            req[kHeadersCountSymbol] = rawHeaders.length;
          }

          const res = new FastServerResponse(req, next.slot);
          res.socket = mockSocket;

          this.emit("request", req, res);
        }
      } catch (err) {
        if (this._rid === rid) {
          this.emit("error", err);
        }
      }
    };

    void runAcceptLoop();

    nextTick(() => this.emit("listening"));
    return this;
  }

  address() {
    if (this._address) {
      return {
        address: this._address.hostname,
        port: this._address.port,
        family: this._address.hostname.includes(":") ? "IPv6" : "IPv4"
      };
    }
    return super.address();
  }

  close(callback: any) {
    if (this._rid !== undefined) {
      const rid = this._rid;
      this._rid = undefined;
      this._address = null;
      core.ops.op_http_shutdown(rid)
        .then(() => {
          this.emit("close");
          if (callback) callback();
        })
        .catch((err: any) => {
          this.emit("error", err);
          if (callback) callback(err);
        });
    } else {
      super.close(callback);
    }
    return this;
  }
}

function createServer(opts: any, requestListener?: ServerHandler) {
  if (typeof opts === "function") {
    requestListener = opts;
    opts = {};
  }
  return new ServerImpl(opts, requestListener);
}

function request(...args: any[]) {
  return new ClientRequest(args[0], args[1], args[2]);
}

function get(...args: any[]) {
  const req = request(args[0], args[1], args[2]);
  req.end();
  return req;
}

// Default max header size matches Node.js default (16 KiB).
// Node reads this from --max-http-header-size; we hardcode it.
const maxHeaderSize = 16_384;

function setMaxIdleHTTPParsers(max: number) {
  validateInteger(max, "max", 1);
  parsers.max = max;
}

return {
  _connectionListener,
  Agent,
  ClientRequest,
  createServer,
  get,
  get globalAgent() {
    return httpAgent.globalAgent;
  },
  set globalAgent(value) {
    httpAgent.setGlobalAgent(value);
  },
  IncomingMessage,
  maxHeaderSize,
  METHODS,
  OutgoingMessage,
  request,
  Server: ServerImpl,
  ServerImpl: ServerImpl,
  ServerResponse,
  setGlobalProxyFromEnv,
  setMaxIdleHTTPParsers,
  STATUS_CODES,
  validateHeaderName,
  validateHeaderValue,
};
})();
