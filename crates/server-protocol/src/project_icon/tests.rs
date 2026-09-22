use serde_json::json;

use super::*;

#[test]
fn accepts_only_automatic_and_upload_sources() {
    assert!(
        serde_json::from_value::<ProjectIconSetRequest>(json!({
            "projectId":"prj_a", "source":{"type":"automatic"}
        }))
        .is_ok()
    );
    assert_eq!(
        serde_json::from_value::<ProjectIconSetRequest>(json!({
            "projectId":"prj_a", "source":{"type":"upload","data":"AA=="}
        }))
        .unwrap()
        .source,
        ProjectIconSource::Upload {
            data: "AA==".to_owned()
        }
    );
    assert!(
        serde_json::from_value::<ProjectIconSetRequest>(json!({
            "projectId":"prj_a", "source":{"type":"url","url":"http://127.0.0.1"}
        }))
        .is_err()
    );
}

#[test]
fn get_result_uses_paseo_icon_shape() {
    assert_eq!(
        serde_json::to_value(ProjectIconGetResult {
            project_id: "prj_a".to_owned(),
            icon: Some(ProjectIconPayload {
                data: "AA==".to_owned(),
                mime_type: "image/png".to_owned(),
            }),
            error: None,
        })
        .unwrap(),
        json!({
            "projectId":"prj_a",
            "icon":{"data":"AA==","mimeType":"image/png"},
            "error":null
        })
    );
}
