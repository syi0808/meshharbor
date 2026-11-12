export const DASHBOARD_ORIGIN = 'http://127.0.0.1:5173';
export const DASHBOARD_MARKER = 'local-ui-v1';

const FRAME_CSP = "frame-ancestors 'none'";

export function createDevSecurityMiddleware() {
  return (request, response, next) => {
    // Apply anti-framing headers to both the UI page and all dev responses.
    response.setHeader('Content-Security-Policy', FRAME_CSP);
    response.setHeader('X-Frame-Options', 'DENY');

    const pathname = request.url?.split(/[?#]/, 1)[0];
    if (pathname !== '/api/v1' && !pathname?.startsWith('/api/v1/')) return next();

    const safeRead = request.method === 'GET' || request.method === 'HEAD';
    const originMatches = request.headers.origin === DASHBOARD_ORIGIN;
    const browserSameOriginWithoutOrigin =
      safeRead && !request.headers.origin && request.headers['sec-fetch-site'] === 'same-origin';
    if (
      request.headers.host !== '127.0.0.1:5173' ||
      (!originMatches && !browserSameOriginWithoutOrigin) ||
      request.headers['x-meshharbor-dashboard'] !== DASHBOARD_MARKER
    ) {
      response.statusCode = 403;
      response.end('MeshHarbor dev API requests must come from the local Studio origin.');
      return;
    }
    next();
  };
}
