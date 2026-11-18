// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

// The glass look's backdrop (gnome-extension/src/glass.js): what lies under the island, captured by Isle
// (src/glass.rs, a quarter size), blurred, and cut to the island's shape and toned in one pass by GNOME's
// shader: its lightness mapped into a band, a bright backdrop held deeper, a sheen and a glow inside the edge,
// what is behind bent in near the edge with its red and blue parting, and the edge catching the light.

/** How deep the glow reaches in from the edge, and how open the island is between these heights (CSS px), as
 * GNOME's island.js has them. */
const GLOW_REACH = 4;
const CLOSED = 48, OPEN = 140;

const VERTEX = `#version 300 es
in vec2 corner;
out vec2 uv;
void main() {
    uv = corner * 0.5 + 0.5;
    uv.y = 1.0 - uv.y;
    gl_Position = vec4(corner, 0.0, 1.0);
}`;

// One direction of a Gaussian blur; the first pass also turns the capture's BGRA into RGBA.
const BLUR = `#version 300 es
precision highp float;
in vec2 uv;
out vec4 color;
uniform sampler2D source;
uniform vec2 step;
uniform float sigma;
uniform bool bgra;
void main() {
    vec4 sum = vec4(0.0);
    float total = 0.0;
    int taps = int(ceil(sigma * 3.0));
    for (int i = -24; i <= 24; i++) {
        if (i < -taps || i > taps)
            continue;
        float weight = exp(-float(i * i) / (2.0 * sigma * sigma));
        sum += texture(source, uv + step * float(i)) * weight;
        total += weight;
    }
    color = sum / total;
    if (bgra)
        color = color.bgra;
}`;

// GNOME's ShapeMask (glass.js FUNCTIONS and CODE), in CSS px, with the blurred backdrop for its texture.
const SHAPE = `#version 300 es
precision highp float;
out vec4 color;
uniform sampler2D backdrop;
uniform vec2 size;
uniform float ratio;
uniform float bottom, left, right, radius, shoulder, crown, reach, drop;
uniform float saturation, lo, span, gamma, deep, sheen, glow;
uniform float knee, lens, band, spread, rim, rimWidth;

// How far p is inside the sides: where the shoulders flare out into the screen's top edge, the side is their curve,
// not the body's straight edge carried on up (that would light a straight line inside each curve).
float isleSides(vec2 p) {
    float l = p.x - left, r = right - p.x;
    if (shoulder > 0.0 && p.y < shoulder) {
        l = length(p - vec2(left - shoulder, shoulder)) - shoulder;
        r = length(p - vec2(right + shoulder, shoulder)) - shoulder;
    }
    return min(l, r);
}
float isleInner(vec2 p) {
    float d = min(isleSides(p), bottom - p.y);
    if (p.y > bottom - radius) {
        vec2 c = vec2(clamp(p.x, left + radius, right - radius), bottom - radius);
        d = min(d, radius - length(p - c));
    }
    if (crown > 0.0) {
        d = min(d, p.y);
        if (p.y < crown) {
            vec2 c = vec2(clamp(p.x, left + crown, right - crown), crown);
            d = min(d, crown - length(p - c));
        }
    }
    return d;
}
vec2 isleNormal(vec2 p) {
    return normalize(vec2(isleInner(p + vec2(1.0, 0.0)) - isleInner(p - vec2(1.0, 0.0)),
        isleInner(p + vec2(0.0, 1.0)) - isleInner(p - vec2(0.0, 1.0))) + 1e-5);
}

void main() {
    vec2 p = vec2(gl_FragCoord.x, size.y * ratio - gl_FragCoord.y) / ratio;
    vec2 uv = p / size;
    color = texture(backdrop, uv);
    p.y -= drop;
    float body = 0.0;
    if (p.x >= left - 0.5 && p.x <= right + 0.5) {
        body = clamp(min(min(p.x - left, right - p.x), bottom - p.y) + 0.5, 0.0, 1.0);
        if (p.y > bottom - radius) {
            vec2 c = vec2(clamp(p.x, left + radius, right - radius), bottom - radius);
            body = min(body, clamp(0.5 - (length(p - c) - radius), 0.0, 1.0));
        }
        if (p.y < crown) {
            vec2 c = vec2(clamp(p.x, left + crown, right - crown), crown);
            body = min(body, clamp(0.5 - (length(p - c) - crown), 0.0, 1.0));
        }
    }
    float flare = 0.0;
    if (shoulder > 0.0 && p.y < shoulder) {
        if (p.x < left && p.x > left - shoulder)
            flare = clamp(length(p - vec2(left - shoulder, shoulder)) - shoulder + 0.5, 0.0, 1.0);
        else if (p.x > right && p.x < right + shoulder)
            flare = clamp(length(p - vec2(right + shoulder, shoulder)) - shoulder + 0.5, 0.0, 1.0);
    }
    float cover = max(body, flare);
    if (cover <= 0.0) {
        color = vec4(0.0);
        return;
    }
    if (lens > 0.0) {
        float edgeIn = isleInner(p);
        if (edgeIn >= 0.0 && edgeIn < band) {
            // Inwards across the edge, most at the rim: there it shows what lies a little further in, as thick glass.
            vec2 n = isleNormal(p);
            vec2 shift = n * lens * pow(1.0 - edgeIn / band, 2.0) / (size + 3.0);
            vec4 bent = texture(backdrop, uv + shift);
            color = vec4(texture(backdrop, uv + shift * (1.0 + spread)).r, bent.g,
                texture(backdrop, uv + shift * (1.0 - spread)).b, bent.a);
        }
    }
    vec3 rgb = color.rgb;
    float luma = dot(rgb, vec3(0.2126, 0.7152, 0.0722));
    rgb = lo + span * pow(max(luma, 0.0), gamma) * (1.0 - deep * smoothstep(knee, 1.0, luma)) + (rgb - luma) * span * saturation;
    rgb += sheen * (1.0 - clamp(p.y / max(bottom, 1.0), 0.0, 1.0));
    float inner = min(isleSides(p), bottom - p.y);
    if (p.y > bottom - radius) {
        vec2 c = vec2(clamp(p.x, left + radius, right - radius), bottom - radius);
        inner = min(inner, radius - length(p - c));
    }
    if (inner >= 0.0)
        rgb += glow * exp(-inner / reach) * (0.3 + 0.7 * p.y / max(bottom, 1.0));
    if (rim > 0.0) {
        // Thick glass's edge catching the light: brightest where it faces up and left, again (less) down and right.
        float e = isleInner(p);
        if (e >= 0.0 && e < rimWidth * 8.0) {
            vec2 facing = -isleNormal(p);
            vec2 light = normalize(vec2(-1.0, -1.25));
            float lit = pow(max(dot(facing, light), 0.0), 1.6) + 0.6 * pow(max(dot(facing, -light), 0.0), 1.6);
            rgb += rim * (0.22 + lit) * (exp(-e / rimWidth) + 0.3 * exp(-e / (rimWidth * 3.5)));
        }
    }
    color = vec4(clamp(rgb, 0.0, 1.0) * cover, cover);
}`;

function program(gl, fragment) {
    const compile = (type, source) => {
        const shader = gl.createShader(type);
        gl.shaderSource(shader, source);
        gl.compileShader(shader);
        if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS))
            throw new Error(gl.getShaderInfoLog(shader));
        return shader;
    };
    const made = gl.createProgram();
    gl.attachShader(made, compile(gl.VERTEX_SHADER, VERTEX));
    gl.attachShader(made, compile(gl.FRAGMENT_SHADER, fragment));
    gl.bindAttribLocation(made, 0, 'corner');
    gl.linkProgram(made);
    if (!gl.getProgramParameter(made, gl.LINK_STATUS))
        throw new Error(gl.getProgramInfoLog(made));
    const uniforms = {};
    const count = gl.getProgramParameter(made, gl.ACTIVE_UNIFORMS);
    for (let i = 0; i < count; i++) {
        const name = gl.getActiveUniform(made, i).name;
        uniforms[name] = gl.getUniformLocation(made, name);
    }
    return {program: made, uniforms};
}

function texture(gl) {
    const made = gl.createTexture();
    gl.bindTexture(gl.TEXTURE_2D, made);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    return made;
}

/** Draws the glass on `canvas` (the island window's size, under the shape's canvas). */
export class GlassBackdrop {
    constructor(canvas) {
        this._canvas = canvas;
        const gl = canvas.getContext('webgl2', {premultipliedAlpha: true, alpha: true, antialias: false});
        if (!gl)
            throw new Error('no WebGL 2');
        this._gl = gl;
        this._blur = program(gl, BLUR);
        this._shape = program(gl, SHAPE);
        const quad = gl.createBuffer();
        gl.bindBuffer(gl.ARRAY_BUFFER, quad);
        gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 1, -1, -1, 1, 1, 1]), gl.STATIC_DRAW);
        gl.enableVertexAttribArray(0);
        gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 0, 0);
        this._source = texture(gl);
        this._passes = [texture(gl), texture(gl)];
        this._buffers = this._passes.map(made => {
            const buffer = gl.createFramebuffer();
            gl.bindFramebuffer(gl.FRAMEBUFFER, buffer);
            gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, made, 0);
            return buffer;
        });
        gl.bindFramebuffer(gl.FRAMEBUFFER, null);
        this._frame = null;
        this._radius = 14;
        this._blurred = false;
    }

    /** Whether a picture of what is behind has come. */
    get ready() {
        return this._frame !== null;
    }

    /** A new picture behind the island: `width` × `height` BGRA. */
    setFrame(width, height, pixels) {
        const gl = this._gl;
        gl.bindTexture(gl.TEXTURE_2D, this._source);
        gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
        gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA8, width, height, 0, gl.RGBA, gl.UNSIGNED_BYTE, pixels);
        if (!this._frame || this._frame.width !== width || this._frame.height !== height) {
            for (const made of this._passes) {
                gl.bindTexture(gl.TEXTURE_2D, made);
                gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA8, width, height, 0, gl.RGBA, gl.UNSIGNED_BYTE, null);
            }
        }
        this._frame = {width, height};
        this._blurred = false;
    }

    /** Forgets the picture (glass turned off). */
    clear() {
        this._frame = null;
        const gl = this._gl;
        gl.bindFramebuffer(gl.FRAMEBUFFER, null);
        gl.clearColor(0, 0, 0, 0);
        gl.clear(gl.COLOR_BUFFER_BIT);
    }

    /** Blurs the picture `radius` CSS px (GNOME Shell's blur radius, a Gaussian of half that), in two passes. */
    _blurFrame(radius) {
        const gl = this._gl, {width, height} = this._frame;
        // The picture is a quarter of the window's device pixels on each side.
        const scale = width / Math.max(1, window.innerWidth);
        const sigma = Math.max(0.5, Math.min(8, radius / 2 * scale));
        gl.useProgram(this._blur.program);
        gl.uniform1f(this._blur.uniforms.sigma, sigma);
        gl.viewport(0, 0, width, height);
        gl.disable(gl.BLEND);
        gl.disable(gl.SCISSOR_TEST);
        const pass = (from, to, step, bgra) => {
            gl.bindFramebuffer(gl.FRAMEBUFFER, this._buffers[to]);
            gl.activeTexture(gl.TEXTURE0);
            gl.bindTexture(gl.TEXTURE_2D, from);
            gl.uniform1i(this._blur.uniforms.source, 0);
            gl.uniform2f(this._blur.uniforms.step, step[0], step[1]);
            gl.uniform1i(this._blur.uniforms.bgra, bgra ? 1 : 0);
            gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
        };
        pass(this._source, 0, [1 / width, 0], true);
        pass(this._passes[0], 1, [0, 1 / height], false);
        this._blurred = true;
    }

    /** Draws the glass in `shape` ({cx, w, h, r, s}, as island.js has it), toned by `tone` (theme.js `blur()`). */
    render(shape, tone) {
        const gl = this._gl, canvas = this._canvas;
        const ratio = window.devicePixelRatio || 1;
        const width = window.innerWidth, height = window.innerHeight;
        if (canvas.width !== Math.round(width * ratio) || canvas.height !== Math.round(height * ratio)) {
            canvas.width = Math.round(width * ratio);
            canvas.height = Math.round(height * ratio);
        }
        if (!this._frame || !tone)
            return;
        if (!this._blurred || this._radius !== tone.radius) {
            this._radius = tone.radius;
            this._blurFrame(tone.radius);
        }
        gl.bindFramebuffer(gl.FRAMEBUFFER, null);
        gl.viewport(0, 0, canvas.width, canvas.height);
        gl.disable(gl.SCISSOR_TEST);
        gl.clearColor(0, 0, 0, 0);
        gl.clear(gl.COLOR_BUFFER_BIT);
        if (shape.w <= 0 || shape.h <= 0)
            return;
        // Only the island's box is drawn.
        const x0 = Math.max(0, Math.floor((shape.cx - shape.w / 2 - shape.s - 1) * ratio));
        const x1 = Math.min(canvas.width, Math.ceil((shape.cx + shape.w / 2 + shape.s + 1) * ratio));
        const y1 = Math.min(canvas.height, Math.ceil((shape.h + 1) * ratio));
        gl.enable(gl.SCISSOR_TEST);
        gl.scissor(x0, canvas.height - y1, x1 - x0, y1);
        const u = this._shape.uniforms;
        gl.useProgram(this._shape.program);
        gl.activeTexture(gl.TEXTURE0);
        gl.bindTexture(gl.TEXTURE_2D, this._passes[1]);
        gl.uniform1i(u.backdrop, 0);
        gl.uniform2f(u.size, width, height);
        gl.uniform1f(u.ratio, ratio);
        const open = Math.max(0, Math.min(1, (shape.h - CLOSED) / (OPEN - CLOSED)));
        const values = {
            bottom: shape.h, left: shape.cx - shape.w / 2, right: shape.cx + shape.w / 2, radius: shape.r,
            shoulder: shape.s, crown: 0, reach: GLOW_REACH, drop: 0,
            saturation: tone.small + (tone.large - tone.small) * open, lo: tone.lo, span: tone.span, gamma: tone.gamma,
            deep: tone.deep ?? 0, sheen: tone.sheen, glow: tone.glow,
            knee: tone.knee ?? 0.55, lens: tone.lens ?? 0, band: tone.band ?? 1, spread: tone.spread ?? 0,
            rim: tone.rim ?? 0, rimWidth: tone.rimWidth ?? 1,
        };
        for (const [name, value] of Object.entries(values))
            gl.uniform1f(u[name], value);
        gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
    }
}
