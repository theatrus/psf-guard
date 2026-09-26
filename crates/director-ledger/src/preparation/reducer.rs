use super::*;
use psf_guard_director_core::geometry::GeometryPreparation;

// Both modes use the same journal/transaction lifecycle. Only the shared core
// owns their operation policy and checkpoint validation.
pub(super) enum Reducer<'a> {
    Legacy(Box<Preparation>),
    Geometry(Box<GeometryPreparation<'a>>),
}

macro_rules! access {
    ($name:ident, $result:ty) => {
        pub(super) fn $name(&self) -> $result {
            match self {
                Self::Legacy(inner) => inner.$name(),
                Self::Geometry(inner) => inner.$name(),
            }
        }
    };
}

impl<'a> Reducer<'a> {
    access!(id, &str);
    access!(context, &Context);
    access!(estimates, Estimates);
    access!(pending, Option<&Command>);
    access!(halted, Option<&Decision>);
    access!(observations, &[Observation]);
    access!(steps_completed, bool);

    pub(super) fn restore(
        bytes: &[u8],
        geometry: Option<&'a BoundGeometry>,
    ) -> Result<Self, Error> {
        match geometry {
            Some(geometry) => geometry
                .restore_preparation(bytes)
                .map(Box::new)
                .map(Self::Geometry)
                .map_err(|_| Error::CorruptLedger),
            None => Preparation::restore(bytes)
                .map(Box::new)
                .map(Self::Legacy)
                .map_err(|_| Error::CorruptLedger),
        }
    }

    pub(super) fn checkpoint(&self) -> Result<Vec<u8>, Error> {
        match self {
            Self::Legacy(inner) => inner.checkpoint().map_err(Error::Preparation),
            Self::Geometry(inner) => inner.checkpoint().map_err(Error::Geometry),
        }
    }

    pub(super) fn next(
        &mut self,
        request: &Request,
        current: Option<&Constraints>,
    ) -> Result<Next, Error> {
        match (self, current) {
            (Self::Legacy(inner), None) => inner.next(request).map_err(Error::Preparation),
            (Self::Geometry(inner), Some(current)) => {
                inner.next(request, current).map_err(Error::Geometry)
            }
            _ => Err(Error::ConflictingEvidence),
        }
    }

    pub(super) fn complete(&mut self, completion: Completion) -> Result<(), Error> {
        match self {
            Self::Legacy(inner) => inner.complete(completion).map_err(Error::Preparation),
            Self::Geometry(inner) => inner.complete(completion).map_err(Error::Geometry),
        }
    }
}
