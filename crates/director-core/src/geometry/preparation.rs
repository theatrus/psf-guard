//! Preparation stays bound to the same computed geometry for its whole lifetime.

use super::{BoundGeometry, Constraints, Error};
use crate::preparation::{Command, Completion, Context, Estimates, Next, Observation, Preparation};
use crate::program::LocalState;
use crate::{Decision, Request};

/// Geometry-aware native operation policy, not durable execution authority.
/// The inner reducer cannot be extracted to bypass current constraint checks.
/// Hosts must still journal commands before dispatch and reserve captures.
#[derive(Debug)]
pub struct GeometryPreparation<'a> {
    geometry: &'a BoundGeometry,
    inner: Preparation,
}

impl BoundGeometry {
    pub fn preparation(
        &self,
        id: String,
        request: &Request,
        current: &Constraints,
        goal_id: &str,
        local: LocalState,
        estimates: Estimates,
    ) -> Result<GeometryPreparation<'_>, Error> {
        let narrowed = self.narrow(request)?;
        self.check_current(request, current)?;
        let context = self
            .source
            .preparation_context(goal_id, local)
            .map_err(Error::Program)?;
        let inner =
            Preparation::new(id, &narrowed, context, estimates).map_err(Error::Preparation)?;
        Ok(GeometryPreparation {
            geometry: self,
            inner,
        })
    }
}

impl GeometryPreparation<'_> {
    /// Supply original program intent with projected progress and fresh state.
    /// Remaining setup plus exposure must fit one computed window. Any changed
    /// constraint latches a check-in even if the next snapshot changes back.
    /// In-flight actions retain their receipt obligation; safety stops still win.
    pub fn next(&mut self, request: &Request, current: &Constraints) -> Result<Next, Error> {
        let narrowed = self.geometry.narrow(request)?;
        let changed = self.geometry.check_current(request, current).is_err();
        self.inner
            .next_with_constraint_change(&narrowed, changed)
            .map_err(Error::Preparation)
    }

    /// Receipts remain admissible after a constraint change or safety stop.
    pub fn complete(&mut self, completion: Completion) -> Result<(), Error> {
        self.inner.complete(completion).map_err(Error::Preparation)
    }

    pub fn context(&self) -> &Context {
        self.inner.context()
    }

    pub fn pending(&self) -> Option<&Command> {
        self.inner.pending()
    }

    pub fn observations(&self) -> &[Observation] {
        self.inner.observations()
    }

    pub fn halted(&self) -> Option<&Decision> {
        self.inner.halted()
    }
}
