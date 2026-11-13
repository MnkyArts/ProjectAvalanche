use super::{Builder, R, arg};
use crate::avm1::object::{Func, FuncCode, Kind, NativeFn, ObjId, Object, Property, flags};
use crate::avm1::value::{Str, Value, to_int32};
use crate::avm1::{Ctx, Fault};
use crate::geom::{Cxform, Mat2d};

fn accessor(b: &mut Builder, target: ObjId, name: &str, get: NativeFn, set: NativeFn) {
    let mk = |b: &mut Builder, f: NativeFn| {
        let func = Func { code: FuncCode::Native(f), scope: None, base: None };
        Value::Object(b.heap.alloc(Object::new(Value::Object(b.function_proto), Kind::Function(Box::new(func)))))
    };
    let (g, s) = (mk(b, get), mk(b, set));
    if let Some(o) = b.heap.get_mut(target) {
        o.props.insert(
            Str::from(name),
            Property { value: Value::Undefined, flags: flags::DONT_ENUM, accessor: Some(Box::new((g, s))) },
        );
    }
}

pub(crate) struct GeomClasses {
    pub color: ObjId,
    pub color_transform: ObjId,
    pub transform: ObjId,
    pub matrix: ObjId,
    pub point: ObjId,
    pub rectangle: ObjId,
    pub filter: ObjId,
}

pub(crate) fn install(b: &mut Builder, global: ObjId, flash: ObjId) -> GeomClasses {
    let color = b.obj();
    b.class(global, "Color", color_ctor, color);
    b.methods(
        color,
        &[
            ("setRGB", color_set_rgb),
            ("getRGB", color_get_rgb),
            ("setTransform", color_set_transform),
            ("getTransform", color_get_transform),
        ],
    );
    let geom = b.obj();
    b.value(flash, "geom", Value::Object(geom));
    let ct = b.obj();
    b.class(geom, "ColorTransform", ct_ctor, ct);
    accessor(b, ct, "rgb", ct_get_rgb, ct_set_rgb);
    b.methods(ct, &[("concat", ct_concat), ("toString", |_, _, _| Ok(Value::str("(colorTransform)")))]);
    let transform = b.obj();
    b.class(geom, "Transform", transform_ctor, transform);
    accessor(b, transform, "colorTransform", tr_get_ct, tr_set_ct);
    accessor(b, transform, "matrix", tr_get_matrix, tr_set_matrix);
    let matrix = b.obj();
    b.class(geom, "Matrix", matrix_ctor, matrix);
    b.methods(
        matrix,
        &[
            ("identity", matrix_identity),
            ("clone", matrix_clone),
            ("translate", matrix_translate),
            ("scale", matrix_scale),
        ],
    );
    let point = b.obj();
    b.class(geom, "Point", point_ctor, point);
    b.methods(point, &[("clone", point_clone)]);
    let rect = b.obj();
    b.class(geom, "Rectangle", rect_ctor, rect);
    let filters = b.obj();
    b.value(flash, "filters", Value::Object(filters));
    let filter = b.obj();
    b.class(filters, "BitmapFilter", |_, t, _| Ok(t), filter);
    GeomClasses { color, color_transform: ct, transform, matrix, point, rectangle: rect, filter }
}

fn color_ctor(cx: &mut Ctx, this: Value, a: &[Value]) -> R {
    if let Some(o) = this.as_object().and_then(|o| cx.vm.heap.get_mut(o)) {
        o.kind = Kind::Tagged { tag: "Color", target: arg(a, 0) };
    }
    Ok(this)
}

fn target_of(cx: &mut Ctx, this: &Value) -> Option<crate::display::InstanceId> {
    let t = match cx.kind_of(this) {
        Some(Kind::Tagged { target, .. }) => target.clone(),
        _ => return None,
    };
    cx.display_of(&t)
}

fn cxform_of(cx: &Ctx, id: crate::display::InstanceId) -> Cxform {
    cx.player.get(id).map_or(Cxform::IDENTITY, |i| i.cxform)
}

fn color_set_rgb(cx: &mut Ctx, this: Value, a: &[Value]) -> R {
    let Some(id) = target_of(cx, &this) else { return Ok(Value::Undefined) };
    let v = to_int32(cx.to_number(&arg(a, 0))) as u32;
    let mut c = cxform_of(cx, id);
    c.mult[..3].copy_from_slice(&[0.0; 3]);
    c.add[..3].copy_from_slice(&[(v >> 16) & 255, (v >> 8) & 255, v & 255].map(|x| x as f32 / 255.0));
    cx.player.set_cxform(id, c);
    Ok(Value::Undefined)
}

fn color_get_rgb(cx: &mut Ctx, this: Value, _: &[Value]) -> R {
    let Some(id) = target_of(cx, &this) else { return Ok(Value::Undefined) };
    let c = cxform_of(cx, id);
    let [r, g, b] = [c.add[0], c.add[1], c.add[2]].map(|x| (x * 255.0).round().clamp(0.0, 255.0) as u32);
    Ok(Value::Number(f64::from(r << 16 | g << 8 | b)))
}

fn color_set_transform(cx: &mut Ctx, this: Value, a: &[Value]) -> R {
    let Some(id) = target_of(cx, &this) else { return Ok(Value::Undefined) };
    let t = arg(a, 0);
    let mut c = cxform_of(cx, id);
    for (i, (m, o)) in [("ra", "rb"), ("ga", "gb"), ("ba", "bb"), ("aa", "ab")].iter().enumerate() {
        let mv = cx.get_member(&t, m)?;
        if !mv.is_undefined() {
            c.mult[i] = cx.to_number(&mv) as f32 / 100.0;
        }
        let ov = cx.get_member(&t, o)?;
        if !ov.is_undefined() {
            c.add[i] = cx.to_number(&ov) as f32 / 255.0;
        }
    }
    cx.player.set_cxform(id, c);
    Ok(Value::Undefined)
}

fn color_get_transform(cx: &mut Ctx, this: Value, _: &[Value]) -> R {
    let Some(id) = target_of(cx, &this) else { return Ok(Value::Undefined) };
    let c = cxform_of(cx, id);
    let o = Value::Object(cx.new_object());
    for (i, (m, a)) in [("ra", "rb"), ("ga", "gb"), ("ba", "bb"), ("aa", "ab")].iter().enumerate() {
        cx.set_member(&o, m, Value::Number(f64::from(c.mult[i]) * 100.0))?;
        cx.set_member(&o, a, Value::Number(f64::from(c.add[i]) * 255.0))?;
    }
    Ok(o)
}

const CT_FIELDS: [&str; 8] = [
    "redMultiplier",
    "greenMultiplier",
    "blueMultiplier",
    "alphaMultiplier",
    "redOffset",
    "greenOffset",
    "blueOffset",
    "alphaOffset",
];

fn ct_ctor(cx: &mut Ctx, this: Value, a: &[Value]) -> R {
    for (i, f) in CT_FIELDS.iter().enumerate() {
        let d = if i < 4 { 1.0 } else { 0.0 };
        let v = a.get(i).map_or(d, |v| cx.to_number(v));
        cx.set_member(&this, f, Value::Number(v))?;
    }
    Ok(this)
}

fn ct_read(cx: &mut Ctx, v: &Value) -> Result<Cxform, Fault> {
    let mut c = Cxform::IDENTITY;
    for (i, f) in CT_FIELDS.iter().enumerate() {
        let x = cx.get_member(v, f)?;
        let n = cx.to_number(&x) as f32;
        if n.is_finite() {
            if i < 4 { c.mult[i] = n } else { c.add[i - 4] = n / 255.0 }
        }
    }
    Ok(c)
}

fn ct_write(cx: &mut Ctx, c: &Cxform) -> R {
    let o = Value::Object(cx.new_object_with(cx.vm.protos.color_transform, Kind::Plain));
    for (i, f) in CT_FIELDS.iter().enumerate() {
        let v = if i < 4 { f64::from(c.mult[i]) } else { f64::from(c.add[i - 4]) * 255.0 };
        cx.set_member(&o, f, Value::Number(v))?;
    }
    Ok(o)
}

fn ct_get_rgb(cx: &mut Ctx, this: Value, _: &[Value]) -> R {
    let c = ct_read(cx, &this)?;
    let [r, g, b] = [c.add[0], c.add[1], c.add[2]].map(|x| (x * 255.0).round().clamp(0.0, 255.0) as u32);
    Ok(Value::Number(f64::from(r << 16 | g << 8 | b)))
}

fn ct_set_rgb(cx: &mut Ctx, this: Value, a: &[Value]) -> R {
    let v = to_int32(cx.to_number(&arg(a, 0))) as u32;
    for (i, f) in CT_FIELDS.iter().enumerate().take(3) {
        cx.set_member(&this, f, Value::Number(0.0))?;
        let off = f64::from((v >> (16 - 8 * i as u32)) & 255);
        cx.set_member(&this, CT_FIELDS[i + 4], Value::Number(off))?;
        let _ = f;
    }
    Ok(Value::Undefined)
}

fn ct_concat(cx: &mut Ctx, this: Value, a: &[Value]) -> R {
    let (x, y) = (ct_read(cx, &this)?, ct_read(cx, &arg(a, 0))?);
    let c = x.concat(&y);
    for (i, f) in CT_FIELDS.iter().enumerate() {
        let v = if i < 4 { f64::from(c.mult[i]) } else { f64::from(c.add[i - 4]) * 255.0 };
        cx.set_member(&this, f, Value::Number(v))?;
    }
    Ok(Value::Undefined)
}

fn transform_ctor(cx: &mut Ctx, this: Value, a: &[Value]) -> R {
    if let Some(o) = this.as_object().and_then(|o| cx.vm.heap.get_mut(o)) {
        o.kind = Kind::Tagged { tag: "Transform", target: arg(a, 0) };
    }
    Ok(this)
}

fn tr_get_ct(cx: &mut Ctx, this: Value, _: &[Value]) -> R {
    let Some(id) = target_of(cx, &this) else { return Ok(Value::Undefined) };
    let c = cxform_of(cx, id);
    ct_write(cx, &c)
}

fn tr_set_ct(cx: &mut Ctx, this: Value, a: &[Value]) -> R {
    let Some(id) = target_of(cx, &this) else { return Ok(Value::Undefined) };
    let c = ct_read(cx, &arg(a, 0))?;
    cx.player.set_cxform(id, c);
    Ok(Value::Undefined)
}

fn matrix_values(cx: &mut Ctx, v: &Value) -> Result<Mat2d, Fault> {
    let mut f = [1.0f32, 0.0, 0.0, 1.0, 0.0, 0.0];
    for (i, k) in ["a", "b", "c", "d", "tx", "ty"].iter().enumerate() {
        let x = cx.get_member(v, k)?;
        if !x.is_undefined() {
            f[i] = cx.to_number(&x) as f32;
        }
    }
    Ok(Mat2d { a: f[0], b: f[1], c: f[2], d: f[3], tx: f[4] * 20.0, ty: f[5] * 20.0 })
}

fn matrix_object(cx: &mut Ctx, m: &Mat2d) -> R {
    let o = Value::Object(cx.new_object_with(cx.vm.protos.matrix, Kind::Plain));
    for (k, v) in [("a", m.a), ("b", m.b), ("c", m.c), ("d", m.d), ("tx", m.tx / 20.0), ("ty", m.ty / 20.0)] {
        cx.set_member(&o, k, Value::Number(f64::from(v)))?;
    }
    Ok(o)
}

fn tr_get_matrix(cx: &mut Ctx, this: Value, _: &[Value]) -> R {
    let Some(id) = target_of(cx, &this) else { return Ok(Value::Undefined) };
    let m = cx.player.get(id).map_or(Mat2d::IDENTITY, |i| i.matrix);
    matrix_object(cx, &m)
}

fn tr_set_matrix(cx: &mut Ctx, this: Value, a: &[Value]) -> R {
    let Some(id) = target_of(cx, &this) else { return Ok(Value::Undefined) };
    let m = matrix_values(cx, &arg(a, 0))?;
    cx.player.set_matrix(id, m);
    Ok(Value::Undefined)
}

fn matrix_ctor(cx: &mut Ctx, this: Value, a: &[Value]) -> R {
    for (i, k) in ["a", "b", "c", "d", "tx", "ty"].iter().enumerate() {
        let d = if i == 0 || i == 3 { 1.0 } else { 0.0 };
        let v = a.get(i).map_or(d, |v| cx.to_number(v));
        cx.set_member(&this, k, Value::Number(v))?;
    }
    Ok(this)
}

fn matrix_identity(cx: &mut Ctx, this: Value, _: &[Value]) -> R {
    matrix_ctor(cx, this, &[])?;
    Ok(Value::Undefined)
}

fn matrix_clone(cx: &mut Ctx, this: Value, _: &[Value]) -> R {
    let m = matrix_values(cx, &this)?;
    matrix_object(cx, &m)
}

fn matrix_translate(cx: &mut Ctx, this: Value, a: &[Value]) -> R {
    let (dx, dy) = (cx.to_number(&arg(a, 0)), cx.to_number(&arg(a, 1)));
    for (k, d) in [("tx", dx), ("ty", dy)] {
        let v = cx.get_member(&this, k)?;
        let n = cx.to_number(&v) + d;
        cx.set_member(&this, k, Value::Number(n))?;
    }
    Ok(Value::Undefined)
}

fn matrix_scale(cx: &mut Ctx, this: Value, a: &[Value]) -> R {
    let (sx, sy) = (cx.to_number(&arg(a, 0)), cx.to_number(&arg(a, 1)));
    for (k, s) in [("a", sx), ("b", sy), ("c", sx), ("d", sy), ("tx", sx), ("ty", sy)] {
        let v = cx.get_member(&this, k)?;
        let n = cx.to_number(&v) * s;
        cx.set_member(&this, k, Value::Number(n))?;
    }
    Ok(Value::Undefined)
}

fn point_ctor(cx: &mut Ctx, this: Value, a: &[Value]) -> R {
    let x = a.first().map_or(0.0, |v| cx.to_number(v));
    let y = a.get(1).map_or(0.0, |v| cx.to_number(v));
    cx.set_member(&this, "x", Value::Number(x))?;
    cx.set_member(&this, "y", Value::Number(y))?;
    Ok(this)
}

fn point_clone(cx: &mut Ctx, this: Value, _: &[Value]) -> R {
    let (x, y) = (cx.get_member(&this, "x")?, cx.get_member(&this, "y")?);
    let o = Value::Object(cx.new_object_with(cx.vm.protos.point, Kind::Plain));
    point_ctor(cx, o.clone(), &[x, y])?;
    Ok(o)
}

fn rect_ctor(cx: &mut Ctx, this: Value, a: &[Value]) -> R {
    for (i, k) in ["x", "y", "width", "height"].iter().enumerate() {
        let v = a.get(i).map_or(0.0, |v| cx.to_number(v));
        cx.set_member(&this, k, Value::Number(v))?;
    }
    Ok(this)
}
