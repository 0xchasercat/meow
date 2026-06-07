// GENERATED — do not edit (run: meow types)
export interface NetAddr {
    readonly hostname: string;
    readonly port: number;
}
export interface ServeOptions {
    /** TCP port to bind. Default 8000. */
    port?: number;
    /** Interface to bind. Default "0.0.0.0". */
    hostname?: string;
    /** Abort to stop accepting and drain in-flight requests. */
    signal?: AbortSignal;
    /** Called once the listener is bound, with the resolved address. */
    onListen?: (addr: NetAddr) => void;
}
export interface Server {
    /** The bound address (resolved port - `0` becomes the OS-assigned port). */
    readonly addr: NetAddr;
    /** Stop accepting, drain in-flight requests, release the listener. Idempotent. */
    shutdown(): Promise<void>;
    /** Resolves when the accept loop has fully stopped (after shutdown / signal). */
    readonly finished: Promise<void>;
}
export type Handler = (request: Request) => Response | Promise<Response>;
/**
 * Start a Web-standard HTTP/1.1 server. `handler` is invoked per request with a
 * `Request` and must return a `Response`. Requires the network capability; with no
 * grant the bind rejects through `server.finished` with a fix-pointing error.
 */
export declare function serve(handler: Handler, options?: ServeOptions): Server;
