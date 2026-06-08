import {
  ClientRequest,
  IncomingMessage,
  METHODS,
  Server,
  ServerResponse,
  requestWithProtocol,
  type RequestOptions,
} from "node:http";

type RequestCallback = (response: IncomingMessage) => void;
type RequestInput = string | URL | RequestOptions;
type NodeError = Error & { code?: string };

function tlsServerUnsupported(): NodeError {
  const error = new Error("node:https.createServer requires TLS server support; node:http.createServer is available for local dev servers") as NodeError;
  error.code = "ERR_MEOW_HTTPS_SERVER_UNSUPPORTED";
  return error;
}

export function createServer(): never {
  throw tlsServerUnsupported();
}

export function request(input: RequestInput, optionsOrCallback?: RequestOptions | RequestCallback, maybeCallback?: RequestCallback): ClientRequest {
  return requestWithProtocol("https:", input, optionsOrCallback, maybeCallback);
}

export function get(input: RequestInput, optionsOrCallback?: RequestOptions | RequestCallback, maybeCallback?: RequestCallback): ClientRequest {
  const req = request(input, optionsOrCallback as RequestOptions | RequestCallback | undefined, maybeCallback);
  req.end();
  return req;
}

export { ClientRequest, IncomingMessage, METHODS, Server, ServerResponse };

export default {
  METHODS,
  ClientRequest,
  IncomingMessage,
  Server,
  ServerResponse,
  createServer,
  request,
  get,
};
