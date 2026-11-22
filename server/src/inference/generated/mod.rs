//! Generated model plans.
//!
//! Regenerate with `python3 tools/models/export_plan.py`.

mod det;
mod doc_ori;
mod rec;
mod textline_ori;
mod yunet;

use super::ModelKind;
use super::plan::ModelPlan;

pub(super) fn plan(kind: ModelKind) -> &'static ModelPlan {
    match kind {
        ModelKind::YuNet => &yunet::PLAN,
        ModelKind::OcrDet => &det::PLAN,
        ModelKind::OcrRec => &rec::PLAN,
        ModelKind::DocOri => &doc_ori::PLAN,
        ModelKind::TextlineOri => &textline_ori::PLAN,
    }
}
