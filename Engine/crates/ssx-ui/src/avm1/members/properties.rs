use super::*;

impl Ctx<'_> {
    pub(super) fn primitive_proto(&self, v: &Value) -> Option<ObjId> {
        match v {
            Value::Bool(_) => Some(self.vm.protos.boolean),
            Value::Number(_) => Some(self.vm.protos.number),
            Value::String(_) => Some(self.vm.protos.string),
            _ => None,
        }
    }

    pub fn get_member(&mut self, obj: &Value, name: &str) -> Result<Value, Fault> {
        Ok(self.get_method(obj, name)?.0)
    }

    pub fn get_method(&mut self, obj: &Value, name: &str) -> Result<(Value, Value), Fault> {
        match obj {
            Value::Undefined | Value::Null => Ok((Value::Undefined, Value::Undefined)),
            Value::String(s) if name == "length" => Ok((Value::Number(s.chars().count() as f64), Value::Undefined)),
            Value::Bool(_) | Value::Number(_) | Value::String(_) => {
                let Some(p) = self.primitive_proto(obj) else { return Ok((Value::Undefined, Value::Undefined)) };
                self.lookup_from(&Value::Object(p), obj, name)
            }
            Value::Object(id) => {
                let id = self.resolve_handle(*id);
                if let Some(v) = self.own_special(id, name)? {
                    return Ok((v, Value::Object(id)));
                }
                self.lookup_from(&Value::Object(id), &Value::Object(id), name)
            }
        }
    }

    pub fn peek_member(&self, obj: &Value, name: &str) -> Option<Value> {
        let id = obj.as_object()?;
        let o = self.vm.heap.get(id)?;
        if let Kind::Display(d) = &o.kind {
            if self.player.get(d.id).is_none() || crate::avm1::display::is_display_property(name) {
                return None;
            }
            if !o.props.contains(name)
                && let Some(child) = self.child_named(d.id, name)
            {
                return self
                    .vm
                    .display_objs
                    .get(&child)
                    .filter(|c| self.vm.heap.get(**c).is_some())
                    .map(|c| Value::Object(*c));
            }
        }
        let mut cur = Some(id);
        for _ in 0..MAX_CHAIN {
            let o = self.vm.heap.get(cur?)?;
            if let Some(p) = o.props.get(name) {
                return p.accessor.is_none().then(|| p.value.clone());
            }
            cur = o.proto.as_object();
        }
        None
    }

    pub(super) fn own_special(&mut self, id: ObjId, name: &str) -> Result<Option<Value>, Fault> {
        let Some(o) = self.vm.heap.get(id) else { return Ok(None) };
        if name == "__proto__" {
            return Ok(Some(o.proto.clone()));
        }
        match &o.kind {
            Kind::Array(a) => {
                if name == "length" {
                    return Ok(Some(Value::Number(a.len() as f64)));
                }
                if let Some(i) = array_index(name) {
                    return Ok(Some(a.get(i).cloned().unwrap_or_default()));
                }
            }
            Kind::Boxed(Value::String(s)) if name == "length" => {
                return Ok(Some(Value::Number(s.chars().count() as f64)));
            }
            Kind::Display(d) => {
                let (inst, path) = (d.id, d.path.clone());
                if let Some(v) = self.display_get(inst, name)? {
                    return Ok(Some(v));
                }
                // TODO(guess): `_parent` of a removed clip resolves from its path (Scaleform keeps the parent while `onUnload` runs, `View.onUnload` → `MovieLoader.OnMovieActionComplete`); settle: the character's parent pointer after removal.
                if self.player.get(inst).is_none() && name.eq_ignore_ascii_case("_parent") {
                    let path = path.to_string();
                    if let Some((parent, _)) = path.rsplit_once('.')
                        && let Some(p) = self.instance_at_path(parent)
                    {
                        return Ok(Some(Value::Object(self.object_for(p))));
                    }
                }
                let has_own = self.vm.heap.get(id).is_some_and(|o| o.props.contains(name));
                if !has_own && let Some(c) = self.child_named(inst, name) {
                    return Ok(Some(Value::Object(self.object_for(c))));
                }
            }
            _ => {}
        }
        Ok(None)
    }

    pub fn lookup_from(&mut self, start: &Value, this: &Value, name: &str) -> Result<(Value, Value), Fault> {
        let mut cur = start.as_object();
        for _ in 0..MAX_CHAIN {
            let Some(id) = cur else { break };
            let Some(o) = self.vm.heap.get(id) else { break };
            if let Some(p) = o.props.get(name) {
                return match &p.accessor {
                    Some(acc) => {
                        let getter = acc.0.clone();
                        let v = self.call(&getter, this.clone(), &[], Value::Object(id))?;
                        Ok((v, Value::Object(id)))
                    }
                    None => Ok((p.value.clone(), Value::Object(id))),
                };
            }
            cur = o.proto.as_object();
        }
        if name != "__resolve" {
            let (r, home) = self.lookup_from(start, this, "__resolve")?;
            if self.is_function(&r) {
                let v = self.call(&r, this.clone(), &[Value::str(name)], home)?;
                return Ok((v, Value::Undefined));
            }
        }
        Ok((Value::Undefined, Value::Undefined))
    }

    pub fn has_property(&mut self, id: ObjId, name: &str) -> bool {
        let id = self.resolve_handle(id);
        let Some(o) = self.vm.heap.get(id) else { return false };
        match &o.kind {
            Kind::Array(a) if name == "length" || array_index(name).is_some_and(|i| i < a.len()) => return true,
            Kind::Display(d) => {
                let inst = d.id;
                if crate::avm1::display::is_display_property(name) || self.child_named(inst, name).is_some() {
                    return true;
                }
            }
            _ => {}
        }
        let mut cur = Some(id);
        for _ in 0..MAX_CHAIN {
            let Some(c) = cur else { break };
            let Some(o) = self.vm.heap.get(c) else { break };
            if o.props.contains(name) {
                return true;
            }
            cur = o.proto.as_object();
        }
        false
    }

    pub fn set_member(&mut self, obj: &Value, name: &str, v: Value) -> Result<(), Fault> {
        let Value::Object(id) = obj else { return Ok(()) };
        let id = self.resolve_handle(*id);
        let Some(o) = self.vm.heap.get_mut(id) else { return Ok(()) };
        if name == "__proto__" {
            o.proto = v;
            return Ok(());
        }
        match &mut o.kind {
            Kind::Array(a) => {
                let len = a.len();
                if name == "length" {
                    let n = crate::avm1::value::to_int32(primitive_to_number(&v, 7)).max(0) as usize;
                    self.grew_array(n, n.saturating_sub(len))?;
                    if let Some(Kind::Array(a)) = self.vm.heap.get_mut(id).map(|o| &mut o.kind) {
                        a.resize(n, Value::Undefined);
                    }
                    return Ok(());
                }
                if let Some(i) = array_index(name) {
                    if i >= len {
                        self.grew_array(i + 1, i + 1 - len)?;
                    }
                    if let Some(Kind::Array(a)) = self.vm.heap.get_mut(id).map(|o| &mut o.kind) {
                        if i >= a.len() {
                            a.resize(i + 1, Value::Undefined);
                        }
                        a[i] = v;
                    }
                    return Ok(());
                }
            }
            Kind::Display(d) => {
                let inst = d.id;
                if self.display_set(inst, name, &v)? {
                    return Ok(());
                }
            }
            _ => {}
        }
        let mut cur = Some(id);
        for depth in 0..MAX_CHAIN {
            let Some(c) = cur else { break };
            let Some(o) = self.vm.heap.get_mut(c) else { break };
            if let Some(p) = o.props.get_mut(name) {
                if let Some(acc) = &p.accessor {
                    let setter = acc.1.clone();
                    self.call(&setter, obj.clone(), &[v], Value::Object(c))?;
                    return Ok(());
                }
                if depth == 0 {
                    if p.flags & flags::READ_ONLY == 0 {
                        p.value = v;
                    }
                    return Ok(());
                }
                break;
            }
            cur = o.proto.as_object();
        }
        let props = self.vm.heap.get(id).map_or(0, |o| o.props.len());
        if props >= super::super::sandbox::caps::PROPS {
            return Err(Fault::Abort("an object of more than 65 536 properties"));
        }
        self.new_property(name);
        if let Some(o) = self.vm.heap.get_mut(id) {
            o.props.insert(Str::from(name), Property::new(v, 0));
        }
        Ok(())
    }

    pub fn delete_member(&mut self, obj: &Value, name: &str) -> bool {
        let Value::Object(id) = obj else { return false };
        let id = self.resolve_handle(*id);
        let Some(o) = self.vm.heap.get_mut(id) else { return false };
        if let Kind::Array(a) = &mut o.kind
            && let Some(i) = array_index(name)
        {
            if let Some(x) = a.get_mut(i) {
                *x = Value::Undefined;
                return true;
            }
            return false;
        }
        match o.props.get(name) {
            Some(p) if p.flags & flags::DONT_DELETE != 0 => false,
            Some(_) => {
                o.props.remove(name);
                true
            }
            None => false,
        }
    }
}
