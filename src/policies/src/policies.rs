/*
   Unix Azure Entra ID implementation
   Copyright (C) David Mulder <dmulder@samba.org> 2024

   This program is free software; you can redistribute it and/or modify
   it under the terms of the GNU General Public License as published by
   the Free Software Foundation; either version 3 of the License, or
   (at your option) any later version.

   This program is distributed in the hope that it will be useful,
   but WITHOUT ANY WARRANTY; without even the implied warranty of
   MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
   GNU General Public License for more details.

   You should have received a copy of the GNU General Public License
   along with this program.  If not, see <http://www.gnu.org/licenses/>.
*/
use crate::chromium_ext::ChromiumUserCSE;
use crate::compliance_ext::ComplianceCSE;
use crate::cse::CSE;
use crate::scripts_ext::ScriptsCSE;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use himmelblau_unix_common::config::{split_username, HimmelblauConfig};
use regex::Regex;
use reqwest::{header, Url};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::error;

pub trait PolicySetting: Send + Sync {
    fn enabled(&self) -> bool;
    fn class_type(&self) -> PolicyType;
    fn key(&self) -> String;
    fn value(&self) -> Option<ValueType>;
    fn get_compare_pattern(&self) -> String;
}

#[async_trait]
pub trait Policy: Send + Sync {
    fn get_id(&self) -> String;
    fn get_name(&self) -> String;
    async fn load_policy_settings(&mut self, graph_url: &str, access_token: &str) -> Result<bool>;
    fn list_policy_settings(&self, pattern: Regex) -> Result<Vec<Arc<dyn PolicySetting>>>;
    fn clone(&self) -> Arc<dyn Policy>;
}

#[derive(Deserialize, Clone)]
struct ConfigurationPolicy {
    id: String,
    name: String,
    #[serde(skip)]
    policy_definitions: Option<Vec<Arc<dyn PolicySetting>>>,
}

#[async_trait]
impl Policy for ConfigurationPolicy {
    fn get_id(&self) -> String {
        self.id.clone()
    }

    fn get_name(&self) -> String {
        self.name.clone()
    }

    async fn load_policy_settings(&mut self, graph_url: &str, access_token: &str) -> Result<bool> {
        let settings: Vec<ConfigurationPolicySetting> =
            list_config_policy_settings(graph_url, access_token, &self.id).await?;
        let mut res: Vec<Arc<dyn PolicySetting>> = vec![];
        for setting in settings {
            res.push(Arc::new(setting));
        }
        self.policy_definitions = Some(res);
        Ok(true)
    }

    fn list_policy_settings(&self, pattern: Regex) -> Result<Vec<Arc<dyn PolicySetting>>> {
        match &self.policy_definitions {
            Some(policy_definitions) => {
                let mut res: Vec<Arc<dyn PolicySetting>> = vec![];
                for policy_definition in policy_definitions {
                    if pattern.is_match(&policy_definition.get_compare_pattern()) {
                        res.push(policy_definition.clone());
                    }
                }
                Ok(res)
            }
            None => Err(anyhow!("Policy Definitions were not loaded")),
        }
    }

    fn clone(&self) -> Arc<dyn Policy> {
        Arc::new(ConfigurationPolicy {
            id: self.id.clone(),
            name: self.name.clone(),
            policy_definitions: self.policy_definitions.clone(),
        })
    }
}

#[derive(Deserialize)]
struct ConfigurationPolicies {
    value: Vec<ConfigurationPolicy>,
}

async fn list_configuration_policies(
    graph_url: &str,
    access_token: &str,
) -> Result<Vec<ConfigurationPolicy>> {
    let url = Url::parse_with_params(
        &format!("{}/beta/deviceManagement/configurationPolicies", graph_url),
        &[
            ("$select", "name,id"),
            (
                "$filter",
                "(platforms eq 'linux') and (technologies has 'linuxMdm')",
            ),
        ],
    )
    .map_err(|e| anyhow!("{:?}", e))?;
    let client = reqwest::Client::new();
    let resp = client
        .get(url)
        .header(header::AUTHORIZATION, format!("Bearer {}", access_token))
        .send()
        .await?;
    if resp.status().is_success() {
        Ok(resp.json::<ConfigurationPolicies>().await?.value)
    } else {
        Err(anyhow!(resp.status()))
    }
}

async fn get_compliance_policy_assigned(
    graph_url: &str,
    access_token: &str,
    id: &str,
    policy_id: &str,
) -> Result<bool> {
    let url = &format!(
        "{}/beta/deviceManagement/compliancePolicies/{}/assignments",
        graph_url, policy_id
    );
    let client = reqwest::Client::new();
    let resp = client
        .get(url)
        .header(header::AUTHORIZATION, format!("Bearer {}", access_token))
        .send()
        .await?;
    if resp.status().is_success() {
        let assignments = resp.json::<GroupPolicyAssignments>().await?.value;
        parse_assignments(graph_url, access_token, id, policy_id, assignments).await
    } else {
        Err(anyhow!(resp.status()))
    }
}

async fn list_compliance_policy_settings(
    graph_url: &str,
    access_token: &str,
    policy_id: &str,
) -> Result<Vec<ConfigurationPolicySetting>> {
    let url = &format!(
        "{}/beta/deviceManagement/compliancePolicies/{}/settings",
        graph_url, policy_id
    );
    let client = reqwest::Client::new();
    let resp = client
        .get(url)
        .header(header::AUTHORIZATION, format!("Bearer {}", access_token))
        .send()
        .await?;
    if resp.status().is_success() {
        Ok(resp.json::<ConfigurationPoliciesSettings>().await?.value)
    } else {
        Err(anyhow!(resp.status()))
    }
}

#[derive(Deserialize)]
struct CompliancePolicy {
    id: String,
    name: String,
    #[serde(skip)]
    policy_definitions: Option<Vec<Arc<dyn PolicySetting>>>,
}

#[async_trait]
impl Policy for CompliancePolicy {
    fn get_id(&self) -> String {
        self.id.clone()
    }

    fn get_name(&self) -> String {
        self.name.clone()
    }

    async fn load_policy_settings(&mut self, graph_url: &str, access_token: &str) -> Result<bool> {
        let settings: Vec<ConfigurationPolicySetting> =
            list_compliance_policy_settings(graph_url, access_token, &self.id).await?;
        let mut res: Vec<Arc<dyn PolicySetting>> = vec![];
        for setting in settings {
            res.push(Arc::new(setting));
        }
        self.policy_definitions = Some(res);
        Ok(true)
    }

    fn list_policy_settings(&self, pattern: Regex) -> Result<Vec<Arc<dyn PolicySetting>>> {
        match &self.policy_definitions {
            Some(policy_definitions) => {
                let mut res: Vec<Arc<dyn PolicySetting>> = vec![];
                for policy_definition in policy_definitions {
                    if pattern.is_match(&policy_definition.get_compare_pattern()) {
                        res.push(policy_definition.clone());
                    }
                }
                Ok(res)
            }
            None => Err(anyhow!("Policy Definitions were not loaded")),
        }
    }

    fn clone(&self) -> Arc<dyn Policy> {
        Arc::new(CompliancePolicy {
            id: self.id.clone(),
            name: self.name.clone(),
            policy_definitions: self.policy_definitions.clone(),
        })
    }
}

#[derive(Deserialize)]
struct CompliancePolicies {
    value: Vec<CompliancePolicy>,
}

async fn list_compliance_policies(
    graph_url: &str,
    access_token: &str,
) -> Result<Vec<CompliancePolicy>> {
    let url = Url::parse_with_params(
        &format!("{}/beta/deviceManagement/compliancePolicies", graph_url),
        &[
            ("$select", "name,id"),
            (
                "$filter",
                "(platforms eq 'linux') and (technologies has 'linuxMdm')",
            ),
        ],
    )
    .map_err(|e| anyhow!("{:?}", e))?;
    let client = reqwest::Client::new();
    let resp = client
        .get(url)
        .header(header::AUTHORIZATION, format!("Bearer {}", access_token))
        .send()
        .await?;
    if resp.status().is_success() {
        Ok(resp.json::<CompliancePolicies>().await?.value)
    } else {
        Err(anyhow!(resp.status()))
    }
}

#[derive(Debug, Deserialize)]
struct GroupPolicyConfiguration {
    id: String,
    enabled: bool,
}

#[derive(Debug, Deserialize)]
struct GroupPolicyConfigurations {
    value: Vec<GroupPolicyConfiguration>,
}

async fn list_group_policy_configurations(
    graph_url: &str,
    access_token: &str,
    policy_id: &str,
) -> Result<Vec<GroupPolicyConfiguration>> {
    let url = &format!(
        "{}/beta/deviceManagement/groupPolicyConfigurations/{}/definitionValues",
        graph_url, policy_id
    );
    let client = reqwest::Client::new();
    let resp = client
        .get(url)
        .header(header::AUTHORIZATION, format!("Bearer {}", access_token))
        .send()
        .await?;
    if resp.status().is_success() {
        Ok(resp.json::<GroupPolicyConfigurations>().await?.value)
    } else {
        Err(anyhow!(resp.status()))
    }
}

#[derive(Deserialize, Clone)]
struct GroupPolicyDefinition {
    #[serde(skip)]
    enabled: bool,
    #[serde(rename = "classType")]
    class_type: String,
    #[serde(rename = "displayName")]
    display_name: String,
    #[serde(rename = "categoryPath")]
    category_path: String,
    #[serde(skip)]
    value: PresentationValue,
}

impl PolicySetting for GroupPolicyDefinition {
    fn enabled(&self) -> bool {
        self.enabled
    }

    fn class_type(&self) -> PolicyType {
        if self.class_type == "user" {
            PolicyType::User
        } else if self.class_type == "device" {
            PolicyType::Device
        } else {
            PolicyType::Unknown
        }
    }

    fn key(&self) -> String {
        self.display_name.clone()
    }

    fn value(&self) -> Option<ValueType> {
        match &self.value.value {
            Some(value) => Some(value.clone()),
            None => self.value.values.as_ref().cloned(),
        }
    }

    fn get_compare_pattern(&self) -> String {
        self.category_path.clone()
    }
}

async fn get_group_policy_definition(
    graph_url: &str,
    access_token: &str,
    policy_id: &str,
    def_id: &str,
) -> Result<GroupPolicyDefinition> {
    let url = &format!(
        "{}/beta/deviceManagement/groupPolicyConfigurations/{}/definitionValues/{}/definition",
        graph_url, policy_id, def_id
    );
    let client = reqwest::Client::new();
    let resp = client
        .get(url)
        .header(header::AUTHORIZATION, format!("Bearer {}", access_token))
        .send()
        .await?;
    if resp.status().is_success() {
        Ok(resp.json::<GroupPolicyDefinition>().await?)
    } else {
        Err(anyhow!(resp.status()))
    }
}

#[derive(Deserialize, Serialize, Clone)]
#[serde(untagged)]
pub enum ValueType {
    Text(String),
    Decimal(i64),
    Boolean(bool),
    MultiText(Vec<String>),
    List(Vec<PresentationValueList>),
    #[serde(skip)]
    Collection(Vec<Arc<dyn PolicySetting>>),
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct PresentationValueList {
    name: String,
    value: Option<String>,
}

#[derive(Default, Deserialize, Clone)]
struct PresentationValue {
    value: Option<ValueType>,
    values: Option<ValueType>,
}

#[derive(Deserialize)]
struct PresentationValues {
    value: Option<Vec<PresentationValue>>,
}

async fn get_group_policy_values(
    graph_url: &str,
    access_token: &str,
    policy_id: &str,
    definition_id: &str,
) -> Result<PresentationValue> {
    let url = &format!("{}/beta/deviceManagement/groupPolicyConfigurations/{}/definitionValues/{}/presentationValues", graph_url, policy_id, definition_id);
    let client = reqwest::Client::new();
    let resp = client
        .get(url)
        .header(header::AUTHORIZATION, format!("Bearer {}", access_token))
        .send()
        .await?;
    if resp.status().is_success() {
        match resp.json::<PresentationValues>().await?.value {
            Some(value) => {
                // There should be exactly one value
                if value.len() != 1 {
                    Err(anyhow!("The wrong number of values were returned"))
                } else {
                    Ok(value[0].clone())
                }
            }
            None => Err(anyhow!("No values were returned")),
        }
    } else {
        Err(anyhow!(resp.status()))
    }
}

#[derive(Deserialize, Clone)]
pub struct GroupPolicy {
    id: String,
    #[serde(rename = "displayName")]
    name: String,
    #[serde(skip)]
    policy_definitions: Option<Vec<Arc<dyn PolicySetting>>>,
}

#[async_trait]
impl Policy for GroupPolicy {
    fn get_id(&self) -> String {
        self.id.clone()
    }

    fn get_name(&self) -> String {
        self.name.clone()
    }

    async fn load_policy_settings(&mut self, graph_url: &str, access_token: &str) -> Result<bool> {
        let mut res: Vec<Arc<dyn PolicySetting>> = vec![];
        let definition_values =
            list_group_policy_configurations(graph_url, access_token, &self.id).await?;
        for definition_value in definition_values {
            let mut definition = get_group_policy_definition(
                graph_url,
                access_token,
                &self.id,
                &definition_value.id,
            )
            .await?;
            definition.enabled = definition_value.enabled;
            match get_group_policy_values(graph_url, access_token, &self.id, &definition_value.id)
                .await
            {
                Ok(val) => {
                    definition.value = val;
                    res.push(Arc::new(definition));
                }
                Err(e) => {
                    error!(
                        "Failed fetching presentation value for {}: {}",
                        definition_value.id, e
                    );
                }
            };
        }
        self.policy_definitions = Some(res);
        Ok(true)
    }

    fn list_policy_settings(&self, pattern: Regex) -> Result<Vec<Arc<dyn PolicySetting>>> {
        match &self.policy_definitions {
            Some(policy_definitions) => {
                let mut res: Vec<Arc<dyn PolicySetting>> = vec![];
                for policy_definition in policy_definitions {
                    if pattern.is_match(&policy_definition.get_compare_pattern()) {
                        res.push(policy_definition.clone());
                    }
                }
                Ok(res)
            }
            None => Err(anyhow!("Policy Definitions were not loaded")),
        }
    }

    fn clone(&self) -> Arc<dyn Policy> {
        Arc::new(GroupPolicy {
            id: self.id.clone(),
            name: self.name.clone(),
            policy_definitions: self.policy_definitions.clone(),
        })
    }
}

#[derive(Deserialize)]
struct GroupPolicies {
    value: Vec<GroupPolicy>,
}

async fn list_group_policies(graph_url: &str, access_token: &str) -> Result<Vec<GroupPolicy>> {
    let url = Url::parse_with_params(
        &format!(
            "{}/beta/deviceManagement/groupPolicyConfigurations",
            graph_url
        ),
        &[("$select", "displayName,id")],
    )
    .map_err(|e| anyhow!("{:?}", e))?;
    let client = reqwest::Client::new();
    let resp = client
        .get(url)
        .header(header::AUTHORIZATION, format!("Bearer {}", access_token))
        .send()
        .await?;
    if resp.status().is_success() {
        Ok(resp.json::<GroupPolicies>().await?.value)
    } else {
        Err(anyhow!(resp.status()))
    }
}

#[derive(PartialEq)]
pub enum PolicyType {
    User,
    Device,
    Unknown,
}

#[derive(Serialize, Deserialize)]
struct DirectoryObjectsRequest {
    ids: Vec<String>,
    types: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct MemberGroupsRequest {
    #[serde(rename = "groupIds")]
    group_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct MemberGroupsResponse {
    value: Vec<String>,
}

async fn id_memberof_group(
    graph_url: &str,
    access_token: &str,
    id: &str,
    group_id: &str,
) -> Result<bool> {
    let url = &format!(
        "{}/v1.0/directoryObjects/{}/checkMemberGroups",
        graph_url, id
    );
    let client = reqwest::Client::new();

    let json_payload = serde_json::to_string(&MemberGroupsRequest {
        group_ids: vec![group_id.to_string()],
    })?;

    let resp = client
        .post(url)
        .header(header::AUTHORIZATION, format!("Bearer {}", access_token))
        .header(header::CONTENT_TYPE, "application/json")
        .body(json_payload)
        .send()
        .await?;
    if resp.status().is_success() {
        Ok(resp
            .json::<MemberGroupsResponse>()
            .await?
            .value
            .contains(&group_id.to_string()))
    } else {
        Err(anyhow!(resp.status()))
    }
}

#[derive(Debug, Deserialize)]
struct GroupPolicyAssignmentTarget {
    #[serde(rename = "@odata.type")]
    odata_type: String,
    #[serde(rename = "deviceAndAppManagementAssignmentFilterId")]
    filter_id: Option<String>,
    /* #[serde(rename = "deviceAndAppManagementAssignmentFilterType")]
    filter_type: String,*/
    #[serde(rename = "groupId")]
    group_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GroupPolicyAssignment {
    target: GroupPolicyAssignmentTarget,
}

#[derive(Debug, Deserialize)]
struct GroupPolicyAssignments {
    value: Vec<GroupPolicyAssignment>,
}

async fn parse_assignments(
    graph_url: &str,
    access_token: &str,
    id: &str,
    policy_id: &str,
    assignments: Vec<GroupPolicyAssignment>,
) -> Result<bool> {
    let mut assigned = false;
    let mut excluded = false;
    for rule in assignments {
        if rule.target.filter_id.is_some() {
            error!(
                "TODO: Device filters have not been implemented, GPO {} will be disabled",
                policy_id
            );
            return Ok(false);
        }
        match rule.target.odata_type.as_str() {
            "#microsoft.graph.allLicensedUsersAssignmentTarget" => {
                assigned = true;
            }
            "#microsoft.graph.allDevicesAssignmentTarget" => {
                assigned = true;
            }
            "#microsoft.graph.groupAssignmentTarget" => match rule.target.group_id {
                Some(group_id) => {
                    let member_of =
                        id_memberof_group(graph_url, access_token, id, &group_id).await?;
                    if member_of {
                        assigned = true;
                    }
                }
                None => error!("GPO {}: groupAssignmentTarget missing group id", policy_id),
            },
            "#microsoft.graph.exclusionGroupAssignmentTarget" => match rule.target.group_id {
                Some(group_id) => {
                    let member_of =
                        id_memberof_group(graph_url, access_token, id, &group_id).await?;
                    if member_of {
                        excluded = true;
                    }
                }
                None => error!("GPO {}: groupAssignmentTarget missing group id", policy_id),
            },
            target => {
                error!("GPO {}: unrecognized rule target \"{}\"", policy_id, target);
            }
        }
    }
    if assigned && !excluded {
        Ok(true)
    } else {
        Ok(false)
    }
}

async fn get_gpo_assigned(
    graph_url: &str,
    access_token: &str,
    id: &str,
    policy_id: &str,
) -> Result<bool> {
    let url = &format!(
        "{}/beta/deviceManagement/groupPolicyConfigurations/{}/assignments",
        graph_url, policy_id
    );
    let client = reqwest::Client::new();
    let resp = client
        .get(url)
        .header(header::AUTHORIZATION, format!("Bearer {}", access_token))
        .send()
        .await?;
    if resp.status().is_success() {
        let assignments = resp.json::<GroupPolicyAssignments>().await?.value;
        parse_assignments(graph_url, access_token, id, policy_id, assignments).await
    } else {
        Err(anyhow!(resp.status()))
    }
}

async fn get_config_policy_assigned(
    graph_url: &str,
    access_token: &str,
    id: &str,
    policy_id: &str,
) -> Result<bool> {
    let url = &format!(
        "{}/beta/deviceManagement/configurationPolicies/{}/assignments",
        graph_url, policy_id
    );
    let client = reqwest::Client::new();
    let resp = client
        .get(url)
        .header(header::AUTHORIZATION, format!("Bearer {}", access_token))
        .send()
        .await?;
    if resp.status().is_success() {
        let assignments = resp.json::<GroupPolicyAssignments>().await?.value;
        parse_assignments(graph_url, access_token, id, policy_id, assignments).await
    } else {
        Err(anyhow!(resp.status()))
    }
}

#[derive(Debug, Deserialize, Clone)]
struct SimpleSettingValue {
    value: String,
}

#[derive(Debug, Deserialize, Clone)]
struct ChoiceSettingValue {
    value: String,
}

#[derive(Debug, Deserialize, Clone)]
struct GroupSettingCollectionValue {
    children: Vec<SettingInstance>,
}

#[derive(Debug, Deserialize, Clone)]
struct SettingInstance {
    #[serde(rename = "@odata.type")]
    odata_type: String,
    #[serde(rename = "settingDefinitionId")]
    setting_definition_id: String,
    #[serde(rename = "simpleSettingValue", default)]
    simple_value: Option<SimpleSettingValue>,
    #[serde(rename = "choiceSettingValue", default)]
    choice_value: Option<ChoiceSettingValue>,
    #[serde(rename = "groupSettingCollectionValue", default)]
    group_value: Option<Vec<GroupSettingCollectionValue>>,
}

#[derive(Debug, Deserialize)]
struct ConfigurationPolicySetting {
    #[serde(rename = "settingInstance")]
    setting_instance: SettingInstance,
}

pub fn parse_input_value(input: &str) -> ValueType {
    // Attempt to parse the input as a decimal (i64)
    if let Ok(decimal) = input.parse::<i64>() {
        return ValueType::Decimal(decimal);
    }

    // Attempt to parse the input as a boolean
    if input.eq_ignore_ascii_case("true") {
        return ValueType::Boolean(true);
    } else if input.eq_ignore_ascii_case("false") {
        return ValueType::Boolean(false);
    }

    // If neither, treat it as plain text
    ValueType::Text(input.to_string())
}

impl PolicySetting for ConfigurationPolicySetting {
    fn enabled(&self) -> bool {
        // Configuration Policies can't be disabled, so this is always true
        true
    }

    fn class_type(&self) -> PolicyType {
        let user = match Regex::new(r"^user_") {
            Ok(user) => user,
            Err(_e) => return PolicyType::Unknown,
        };
        let device = match Regex::new(r"^device_") {
            Ok(device) => device,
            Err(_e) => return PolicyType::Unknown,
        };
        if user.is_match(&self.setting_instance.setting_definition_id) {
            PolicyType::User
        } else if device.is_match(&self.setting_instance.setting_definition_id) {
            PolicyType::Device
        } else {
            PolicyType::Unknown
        }
    }

    fn key(&self) -> String {
        self.setting_instance.setting_definition_id.to_string()
    }

    fn value(&self) -> Option<ValueType> {
        match self.setting_instance.odata_type.as_str() {
            "#microsoft.graph.deviceManagementConfigurationSimpleSettingInstance" => self
                .setting_instance
                .simple_value
                .clone()
                .map(|val| parse_input_value(&val.value)),
            "#microsoft.graph.deviceManagementConfigurationChoiceSettingInstance" => {
                let def_id = format!("{}_", self.setting_instance.setting_definition_id);
                self.setting_instance
                    .choice_value
                    .as_ref()
                    .and_then(|val| val.value.strip_prefix(&def_id).map(|s| s.to_string()))
                    .as_deref()
                    .map(parse_input_value)
            }
            "#microsoft.graph.deviceManagementConfigurationGroupSettingCollectionInstance" => {
                self.setting_instance.group_value.clone().map(|collection| {
                    ValueType::Collection(
                        collection
                            .into_iter()
                            .flat_map(|sub_collection| {
                                sub_collection.children.into_iter().map(|child| {
                                    Arc::new(ConfigurationPolicySetting {
                                        setting_instance: child.clone(),
                                    }) as Arc<dyn PolicySetting>
                                })
                            })
                            .collect(),
                    )
                })
            }
            unknown => {
                error!(
                    "Unrecognized device management configuration setting instance: {}",
                    unknown
                );
                None
            }
        }
    }

    fn get_compare_pattern(&self) -> String {
        self.setting_instance.setting_definition_id.to_string()
    }
}

#[derive(Debug, Deserialize)]
struct ConfigurationPoliciesSettings {
    value: Vec<ConfigurationPolicySetting>,
}

async fn list_config_policy_settings(
    graph_url: &str,
    access_token: &str,
    policy_id: &str,
) -> Result<Vec<ConfigurationPolicySetting>> {
    let url = &format!(
        "{}/beta/deviceManagement/configurationPolicies/{}/settings",
        graph_url, policy_id
    );
    let client = reqwest::Client::new();
    let resp = client
        .get(url)
        .header(header::AUTHORIZATION, format!("Bearer {}", access_token))
        .send()
        .await?;
    if resp.status().is_success() {
        Ok(resp.json::<ConfigurationPoliciesSettings>().await?.value)
    } else {
        Err(anyhow!(resp.status()))
    }
}

/* get_gpo_list
 * Get the full list of Group Policy Objects for a given id (user or device).
 *
 * graph_url        The microsoft graph URL
 * access_token     An authenticated token for reading the graph
 * id               The ID of the user/group/device to list policies for
 */
async fn get_gpo_list(
    graph_url: &str,
    access_token: &str,
    id: &str,
) -> Result<Vec<Arc<dyn Policy>>> {
    let mut res: Vec<Arc<dyn Policy>> = vec![];
    let config_policy_list = list_configuration_policies(graph_url, access_token).await?;
    for mut policy in config_policy_list {
        // Check assignments and whether this policy applies
        let assigned = get_config_policy_assigned(graph_url, access_token, id, &policy.id).await?;
        if assigned {
            // Only load policy defs if we know we'll be using them
            policy.load_policy_settings(graph_url, access_token).await?;
            res.push(Arc::new(policy));
        }
    }
    let group_policy_list = list_group_policies(graph_url, access_token).await?;
    for mut gpo in group_policy_list {
        // Check assignments and whether this policy applies
        let assigned = get_gpo_assigned(graph_url, access_token, id, &gpo.id).await?;
        if assigned {
            // Only load policy defs if we know we'll be using them
            gpo.load_policy_settings(graph_url, access_token).await?;
            res.push(Arc::new(gpo));
        }
    }
    let compliance_policy_list = list_compliance_policies(graph_url, access_token).await?;
    for mut policy in compliance_policy_list {
        // Check assignments and whether this policy applies
        let assigned =
            get_compliance_policy_assigned(graph_url, access_token, id, &policy.id).await?;
        if assigned {
            // Only load policy defs if we know we'll be using them
            policy.load_policy_settings(graph_url, access_token).await?;
            res.push(Arc::new(policy));
        }
    }
    Ok(res)
}

pub async fn apply_group_policy(
    config: &HimmelblauConfig,
    access_token: &str,
    account_id: &str,
    id: &str,
) -> Result<bool> {
    let domain = split_username(account_id)
        .map(|(_, domain)| domain)
        .ok_or(anyhow!(
            "Failed to parse domain name from account id '{}'",
            account_id
        ))?;
    let graph_url = config
        .get_graph_url(domain)
        .ok_or(anyhow!("Failed to find graph url for domain {}", domain))?;
    let changed_gpos = get_gpo_list(&graph_url, access_token, id).await?;

    let gp_extensions: Vec<Arc<dyn CSE>> = vec![
        Arc::new(ChromiumUserCSE::new(config, account_id)),
        Arc::new(ScriptsCSE::new(config, account_id)),
        Arc::new(ComplianceCSE::new(config, account_id)),
    ];

    for ext in gp_extensions {
        let cchanged_gpos: Vec<Arc<dyn Policy>> = changed_gpos.to_vec();
        ext.process_group_policy(cchanged_gpos).await?;
    }

    Ok(true)
}
