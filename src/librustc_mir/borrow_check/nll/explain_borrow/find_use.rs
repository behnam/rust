// Copyright 2017 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use std::collections::VecDeque;
use std::rc::Rc;

use borrow_check::nll::region_infer::{Cause, RegionInferenceContext};
use borrow_check::nll::ToRegionVid;
use rustc::mir::visit::{MirVisitable, PlaceContext, Visitor};
use rustc::mir::{Local, Location, Mir};
use rustc::ty::{RegionVid, TyCtxt};
use rustc_data_structures::fx::FxHashSet;
use util::liveness::{self, DefUse};

crate fn find<'tcx>(
    mir: &Mir<'tcx>,
    regioncx: &Rc<RegionInferenceContext<'tcx>>,
    tcx: TyCtxt<'_, '_, 'tcx>,
    region_vid: RegionVid,
    start_point: Location,
) -> Option<Cause> {
    let mut uf = UseFinder {
        mir,
        regioncx,
        tcx,
        region_vid,
        start_point,
    };

    uf.find()
}

struct UseFinder<'cx, 'gcx: 'tcx, 'tcx: 'cx> {
    mir: &'cx Mir<'tcx>,
    regioncx: &'cx Rc<RegionInferenceContext<'tcx>>,
    tcx: TyCtxt<'cx, 'gcx, 'tcx>,
    region_vid: RegionVid,
    start_point: Location,
}

impl<'cx, 'gcx, 'tcx> UseFinder<'cx, 'gcx, 'tcx> {
    fn find(&mut self) -> Option<Cause> {
        let mut queue = VecDeque::new();
        let mut visited = FxHashSet::default();

        queue.push_back(self.start_point);
        while let Some(p) = queue.pop_front() {
            if !self.regioncx.region_contains(self.region_vid, p) {
                continue;
            }

            if !visited.insert(p) {
                continue;
            }

            let block_data = &self.mir[p.block];

            match self.def_use(p, block_data.visitable(p.statement_index)) {
                Some(DefUseResult::Def) => {}

                Some(DefUseResult::UseLive { local }) => {
                    return Some(Cause::LiveVar(local, p));
                }

                Some(DefUseResult::UseDrop { local }) => {
                    return Some(Cause::DropVar(local, p));
                }

                None => {
                    if p.statement_index < block_data.statements.len() {
                        queue.push_back(Location {
                            statement_index: p.statement_index + 1,
                            ..p
                        });
                    } else {
                        queue.extend(
                            block_data
                                .terminator()
                                .successors()
                                .filter(|&bb| Some(&Some(*bb)) != block_data.terminator().unwind())
                                .map(|&bb| Location {
                                    statement_index: 0,
                                    block: bb,
                                }),
                        );
                    }
                }
            }
        }

        None
    }

    fn def_use(&self, location: Location, thing: &dyn MirVisitable<'tcx>) -> Option<DefUseResult> {
        let mut visitor = DefUseVisitor {
            mir: self.mir,
            tcx: self.tcx,
            region_vid: self.region_vid,
            def_use_result: None,
        };

        thing.apply(location, &mut visitor);

        visitor.def_use_result
    }
}

struct DefUseVisitor<'cx, 'gcx: 'tcx, 'tcx: 'cx> {
    mir: &'cx Mir<'tcx>,
    tcx: TyCtxt<'cx, 'gcx, 'tcx>,
    region_vid: RegionVid,
    def_use_result: Option<DefUseResult>,
}

enum DefUseResult {
    Def,
    UseLive { local: Local },
    UseDrop { local: Local },
}

impl<'cx, 'gcx, 'tcx> Visitor<'tcx> for DefUseVisitor<'cx, 'gcx, 'tcx> {
    fn visit_local(&mut self, &local: &Local, context: PlaceContext<'tcx>, _: Location) {
        let local_ty = self.mir.local_decls[local].ty;

        let mut found_it = false;
        self.tcx.for_each_free_region(&local_ty, |r| {
            if r.to_region_vid() == self.region_vid {
                found_it = true;
            }
        });

        if found_it {
            self.def_use_result = match liveness::categorize(context) {
                Some(DefUse::Def) => Some(DefUseResult::Def),
                Some(DefUse::Use) => Some(DefUseResult::UseLive { local }),
                Some(DefUse::Drop) => Some(DefUseResult::UseDrop { local }),
                None => None,
            };
        }
    }
}
