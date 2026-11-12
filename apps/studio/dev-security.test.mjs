import assert from 'node:assert/strict';
import test from 'node:test';

import { createDevSecurityMiddleware, DASHBOARD_MARKER, DASHBOARD_ORIGIN } from './dev-security.mjs';

function invoke({ method = 'GET', url = '/api/v1/runtime', headers = {} } = {}) {
  const responseHeaders = new Map();
  const response = {
    statusCode: 200,
    body: '',
    setHeader(name, value) {
      responseHeaders.set(name.toLowerCase(), value);
    },
    end(body = '') {
      this.body = body;
    },
  };
  let nextCalled = false;
  createDevSecurityMiddleware()(
    { method, url, headers },
    response,
    () => {
      nextCalled = true;
    },
  );
  return { response, responseHeaders, nextCalled };
}

test('adds anti-framing headers to UI responses', () => {
  const { responseHeaders, nextCalled } = invoke({ url: '/' });
  assert.equal(nextCalled, true);
  assert.equal(responseHeaders.get('content-security-policy'), "frame-ancestors 'none'");
  assert.equal(responseHeaders.get('x-frame-options'), 'DENY');
});

test('allows only the exact local Studio origin through the dev proxy guard', () => {
  const { response, nextCalled } = invoke({
    headers: {
      host: '127.0.0.1:5173',
      origin: DASHBOARD_ORIGIN,
      'x-meshharbor-dashboard': DASHBOARD_MARKER,
    },
  });
  assert.equal(response.statusCode, 200);
  assert.equal(nextCalled, true);
});

test('allows dashboard writes only with exact Origin and permits originless safe reads only with Fetch Metadata', () => {
  const write = invoke({
    method: 'POST',
    headers: {
      host: '127.0.0.1:5173',
      origin: DASHBOARD_ORIGIN,
      'x-meshharbor-dashboard': DASHBOARD_MARKER,
    },
  });
  assert.equal(write.response.statusCode, 200);
  assert.equal(write.nextCalled, true);

  const safeRead = invoke({
    method: 'GET',
    headers: {
      host: '127.0.0.1:5173',
      'sec-fetch-site': 'same-origin',
      'x-meshharbor-dashboard': DASHBOARD_MARKER,
    },
  });
  assert.equal(safeRead.response.statusCode, 200);
  assert.equal(safeRead.nextCalled, true);
});

test('rejects originless writes, cross-origin preflight and POST, and untrusted Host', () => {
  const cases = [
    { method: 'POST', headers: { host: '127.0.0.1:5173', 'x-meshharbor-dashboard': DASHBOARD_MARKER } },
    {
      method: 'OPTIONS',
      headers: { host: '127.0.0.1:5173', origin: 'http://127.0.0.1:9999', 'x-meshharbor-dashboard': DASHBOARD_MARKER },
    },
    {
      method: 'POST',
      headers: { host: '127.0.0.1:5173', origin: 'http://127.0.0.1:9999', 'x-meshharbor-dashboard': DASHBOARD_MARKER },
    },
    {
      method: 'GET',
      headers: {
        host: 'localhost.evil.test:5173',
        origin: 'http://localhost.evil.test:5173',
        'x-meshharbor-dashboard': DASHBOARD_MARKER,
      },
    },
  ];
  for (const options of cases) {
    const { response, responseHeaders, nextCalled } = invoke(options);
    assert.equal(response.statusCode, 403, JSON.stringify(options));
    assert.equal(nextCalled, false, JSON.stringify(options));
    assert.equal(responseHeaders.get('content-security-policy'), "frame-ancestors 'none'");
    assert.equal(responseHeaders.get('x-frame-options'), 'DENY');
  }
});
