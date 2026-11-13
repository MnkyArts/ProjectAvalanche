use std::sync::Arc;

use ssx_content::{Asset, ContentId, Kind, LoadError, PORT_NAMESPACE, Source};

const FE_BACKGROUND: &str = "fe_background";

pub fn fe_background_id() -> ContentId {
    ContentId::new(PORT_NAMESPACE, Kind::Movie, FE_BACKGROUND).expect("a valid id")
}

pub fn is_slot(id: &ContentId) -> bool {
    id.namespace() == PORT_NAMESPACE && id.kind() == Kind::Movie && id.path() == FE_BACKGROUND
}

pub fn slot_note(id: &ContentId) -> Option<&'static str> {
    is_slot(id).then_some(
        "the port's movie slot behind the globe: the movie plays on the front end in place of the disc's backdrop \
         picture (frontend/textures/fe_background) while the globe shows, covering the frame at its own aspect, \
         looping, without its sound",
    )
}

#[derive(Debug)]
pub struct EmptySlot;

pub struct PortMovies;

impl Source for PortMovies {
    fn name(&self) -> &str {
        PORT_NAMESPACE
    }

    fn has(&self, id: &ContentId) -> bool {
        is_slot(id)
    }

    fn load(&self, id: &ContentId) -> Result<Asset, LoadError> {
        if !is_slot(id) {
            return Err(LoadError::Missing(id.clone()));
        }
        Ok(Asset::Value(Arc::new(EmptySlot)))
    }

    fn list(&self, kind: Kind) -> Vec<ContentId> {
        if kind == Kind::Movie { vec![fe_background_id()] } else { Vec::new() }
    }
}
