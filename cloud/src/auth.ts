import { createRemoteJWKSet, jwtVerify } from "jose";
import { HttpError, type Env } from "./model.ts";
const keys = new Map<string, ReturnType<typeof createRemoteJWKSet>>();
export type Identity = { role: "owner" | "device"; id: string };
export async function identity(request: Request, env: Env): Promise<Identity> {
  if (
    !/^https:\/\/[a-z0-9-]+\.cloudflareaccess\.com$/.test(
      env.ACCESS_TEAM_DOMAIN,
    ) ||
    !env.ACCESS_AUD
  )
    throw new HttpError(503, "Access is not configured");
  const token = request.headers.get("cf-access-jwt-assertion");
  if (!token) throw new HttpError(401, "Sign in with Cloudflare Access");
  try {
    let jwks = keys.get(env.ACCESS_TEAM_DOMAIN);
    if (!jwks) {
      jwks = createRemoteJWKSet(
        new URL(env.ACCESS_TEAM_DOMAIN + "/cdn-cgi/access/certs"),
      );
      keys.set(env.ACCESS_TEAM_DOMAIN, jwks);
    }
    const { payload } = await jwtVerify(token, jwks, {
      issuer: env.ACCESS_TEAM_DOMAIN,
      audience: env.ACCESS_AUD,
      algorithms: ["RS256"],
      requiredClaims: ["exp", "iat", "type"],
    });
    if (payload.type !== "app") throw new Error("Wrong token type");
    if (
      payload.email === env.OWNER_EMAIL &&
      typeof payload.sub === "string" &&
      payload.sub
    )
      return { role: "owner", id: payload.sub };
    if (
      payload.common_name === env.DEVICE_CLIENT_ID &&
      payload.sub === "" &&
      !payload.email
    )
      return { role: "device", id: env.DEVICE_CLIENT_ID };
  } catch {
    throw new HttpError(403, "Access denied");
  }
  throw new HttpError(403, "Access denied");
}
