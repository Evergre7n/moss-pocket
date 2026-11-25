//! 设置窗口里「安全」分区的两个动作。单独成文件，方便不走界面直接测。

use crate::app::MossPocket;
use crate::settings_ui::SettingsForm;

/// 修改密码：两次输入的新密码要一致，旧密码要对。成功后清空表单。
pub(crate) fn change_password(app: &mut MossPocket, form: &mut SettingsForm) -> Result<(), String> {
    let tr = app.tr();
    if form.new_password != form.confirm_password {
        return Err(tr.err_password_mismatch().to_string());
    }
    let session = app.session_mut().ok_or_else(|| tr.err_vault_locked().to_string())?;
    session
        .vault
        .change_password(&form.old_password, &form.new_password)
        .map_err(|error| tr.core_error(&error))?;
    form.old_password.clear();
    form.new_password.clear();
    form.confirm_password.clear();
    Ok(())
}

/// 重新生成恢复密钥，旧的立即作废。新密钥放进表单给用户看。
pub(crate) fn regenerate_key(app: &mut MossPocket, form: &mut SettingsForm) -> Result<(), String> {
    let tr = app.tr();
    let session = app.session_mut().ok_or_else(|| tr.err_vault_locked().to_string())?;
    let key = session.vault.regenerate_recovery_key().map_err(|error| tr.core_error(&error))?;
    form.fresh_key = Some(key);
    Ok(())
}
