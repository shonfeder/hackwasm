use crate::msg::{ConfigResponse, ExecuteMsg, InstantiateMsg, LottoResponse, QueryMsg};
use anybuf::Anybuf;
#[cfg(not(feature = "library"))]
use cosmwasm_std::entry_point;
use cosmwasm_std::{
    ensure_eq, to_binary, Addr, Attribute, BankMsg, Coin, CosmosMsg, Deps, DepsMut, Env,
    MessageInfo, QueryResponse, Response, StdResult, Uint128, WasmMsg,
};
use nois::{NoisCallback, ProxyExecuteMsg};

// use cw2::set_contract_version;

use crate::error::ContractError;
use crate::state::{Config, Lotto, CONFIG, LOTTOS};

const GAME_DURATION: u64 = 300;

/*
// version info for migration info
const CONTRACT_NAME: &str = "crates.io:lotto";
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");
*/

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn instantiate(
    deps: DepsMut,
    _env: Env,
    info: MessageInfo,
    msg: InstantiateMsg,
) -> Result<Response, ContractError> {
    // validate address is correct
    let addr = deps
        .api
        .addr_validate(&info.sender.as_ref())
        .map_err(|_| ContractError::InvalidAddress {})?;

    let proxy = deps
        .api
        .addr_validate(&msg.nois_proxy)
        .map_err(|_| ContractError::InvalidAddress {})?;

    let cnfg = Config {
        manager: addr,
        lotto_nonce: 0,
        nois_proxy: proxy,
    };

    CONFIG.save(deps.storage, &cnfg)?;

    Ok(Response::new()
        .add_attribute("action", "instantiate")
        .add_attribute("manager", info.sender))
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn execute(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: ExecuteMsg,
) -> Result<Response, ContractError> {
    match msg {
        ExecuteMsg::CreateLotto { deposit } => execute_create_lotto(deps, env, info, deposit),
        ExecuteMsg::Deposit { lotto_id } => execute_deposit_lotto(deps, env, info, lotto_id),
        ExecuteMsg::NoisReceive { callback } => execute_receive(deps, env, info, callback),
    }
}

fn execute_create_lotto(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    deposit: Coin,
) -> Result<Response, ContractError> {
    // validate Timestamp
    let mut config = CONFIG.load(deps.storage)?;
    let mut nonce = config.lotto_nonce;

    let expiration = env.block.time.plus_seconds(GAME_DURATION);

    let lotto = Lotto {
        nonce,
        deposit,
        balance: Uint128::new(0),
        depositors: vec![],
        expiration,
        winner: None,
    };
    nonce += 1;

    LOTTOS.save(deps.storage, nonce, &lotto)?;
    config.lotto_nonce = nonce;
    CONFIG.save(deps.storage, &config)?;

    let msg = WasmMsg::Execute {
        contract_addr: config.nois_proxy.into_string(),
        // GetRandomnessAfter requests the randomness from the proxy after a specific timestamp
        // The job id is needed to know what randomness we are referring to upon reception in the callback.
        msg: to_binary(&ProxyExecuteMsg::GetRandomnessAfter {
            after: expiration,
            job_id: "lotto".to_string(),
        })?,
        // We pay here the proxy contract with whatever the depositors sends. The depositor needs to check in advance the proxy prices.
        funds: info.funds, // Just pass on all funds we got
    };

    // save config
    Ok(Response::new()
        .add_message(msg)
        .add_attribute("action", "create_lotto")
        .add_attribute("next_nonce", nonce.to_string()))
}

fn validate_payment(deposit: &Coin, funds: &[Coin]) -> Result<(), ContractError> {
    if funds.is_empty() {
        return Err(ContractError::NoFundsProvided);
    }
    // TODO disallow participant to deposit more than one denom

    for fund in funds {
        if deposit == fund {
            return Ok(());
        }
    }
    Err(ContractError::InvalidPayment)
}

fn execute_deposit_lotto(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    lotto_id: u32,
) -> Result<Response, ContractError> {
    let mut lotto = LOTTOS.load(deps.storage, lotto_id)?;
    let deposit = lotto.clone().deposit;

    // Not sure the best way to go about validating the coin
    validate_payment(&deposit, info.funds.as_slice())?;

    // Check if lotto is active
    if env.block.time >= lotto.expiration {
        return Err(ContractError::InvalidAddress {});
    }
    // Increment total deposit
    let balance: Coin = info
        .clone()
        .funds
        .iter()
        .filter(|coin| coin.denom == deposit.denom)
        .last()
        .unwrap()
        .clone();

    lotto.balance += balance.amount;
    // Add depositor address
    lotto.depositors.push(info.clone().sender);

    // Save the state
    LOTTOS.save(deps.storage, lotto_id, &lotto)?;

    Ok(Response::new()
        .add_attribute("action", "deposit")
        .add_attribute("sender", info.sender.as_ref())
        .add_attribute("new_balance", lotto.balance.to_string()))
}

pub fn execute_receive(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    callback: NoisCallback,
) -> Result<Response, ContractError> {
    let config = CONFIG.load(deps.storage)?;

    // callback should only be allowed to be called by the proxy contract
    // otherwise anyone can cut the randomness workflow and cheat the randomness by sending the randomness directly to this contract
    ensure_eq!(
        info.sender,
        config.nois_proxy,
        ContractError::UnauthorizedReceive
    );
    let randomness: [u8; 32] = callback
        .randomness
        .to_array()
        .map_err(|_| ContractError::InvalidRandomness)?;

    // extract lotto nonce
    let job_id = callback.job_id;
    let lotto_nonce: u32 = job_id
        .strip_prefix("lotto-")
        .expect("Strange, how is the job-id not prefixed with lotto-")
        .parse()
        .unwrap(); //Needs to check that the received nonce is a number

    // Make sure the lotto nonce is valid
    let lotto = LOTTOS.load(deps.storage, lotto_nonce)?;
    assert!(
        lotto.clone().winner.clone().is_some(),
        "Strange, there's already a winner"
    );
    let depositors = lotto.depositors;

    let winner = match nois::pick(randomness, 1, depositors.clone()).first() {
        Some(wn) => wn.clone(),
        None => return Err(ContractError::NoDepositors {}),
    };

    let amount_winner = lotto.balance.mul_floor((50u128, 100)); // 50%
    let amount_community_pool = lotto.balance.mul_floor((50u128, 100)); // 50%
    let denom = lotto.deposit.clone().denom;

    let mut msgs = Vec::<CosmosMsg>::new();

    msgs.push(
        BankMsg::Send {
            to_address: winner.clone().into_string(),
            amount: vec![Coin {
                amount: amount_winner,
                denom: denom.clone(),
            }],
        }
        .into(),
    );

    // Update Lotto Data
    let new_lotto = Lotto {
        nonce: lotto_nonce,
        deposit: lotto.deposit,
        balance: lotto.balance,
        expiration: lotto.expiration,
        depositors: depositors,
        winner: Some(winner.clone()),
    };
    LOTTOS.save(deps.storage, lotto_nonce, &new_lotto)?;

    msgs.push(CosmosMsg::Stargate {
        type_url: "/cosmos.distribution.v1beta1.MsgFundCommunityPool".to_string(),
        value: encode_msg_fund_community_pool(
            &Coin {
                denom: denom.clone(),
                amount: amount_community_pool,
            },
            &env.contract.address,
        )
        .into(),
    });

    Ok(Response::new().add_messages(msgs).add_attributes(vec![
        Attribute::new("action", "receive-randomness-and-send-prize"),
        Attribute::new("winner", winner.to_string()),
        Attribute::new("job_id", job_id),
        Attribute::new(
            "winner_send_amount",
            Coin {
                amount: amount_winner,
                denom,
            }
            .to_string(),
        ), // actual send amount
    ]))
}

fn encode_msg_fund_community_pool(amount: &Coin, depositor: &Addr) -> Vec<u8> {
    // Coin: https://github.com/cosmos/cosmos-sdk/blob/v0.45.15/proto/cosmos/base/v1beta1/coin.proto#L14-L19
    // MsgFundCommunityPool: https://github.com/cosmos/cosmos-sdk/blob/v0.45.15/proto/cosmos/distribution/v1beta1/tx.proto#L69-L76
    let coin = Anybuf::new()
        .append_string(1, &amount.denom)
        .append_string(2, amount.amount.to_string());
    Anybuf::new()
        .append_message(1, &coin)
        .append_string(2, depositor)
        .into_vec()
}

#[entry_point]
pub fn query(deps: Deps, env: Env, msg: QueryMsg) -> StdResult<QueryResponse> {
    let response = match msg {
        QueryMsg::Lotto { lotto_nonce } => to_binary(&query_lotto(deps, env, lotto_nonce)?)?,
        QueryMsg::Config {} => to_binary(&query_config(deps)?)?,
    };
    Ok(response)
}

fn query_lotto(deps: Deps, env: Env, nonce: u32) -> StdResult<LottoResponse> {
    let lotto = LOTTOS.load(deps.storage, nonce)?;
    let winner = match lotto.winner {
        Some(wn) => Some(wn.to_string()),
        None => None,
    };
    let is_expired = env.block.time > lotto.expiration;
    Ok(LottoResponse {
        nonce: lotto.nonce,
        deposit: lotto.deposit,
        balance: lotto.balance,
        depositors: lotto.depositors.iter().map(|dep| dep.to_string()).collect(),
        winner,
        is_expired,
        expiration: lotto.expiration,
    })
}

fn query_config(deps: Deps) -> StdResult<ConfigResponse> {
    let config = CONFIG.load(deps.storage)?;
    Ok(ConfigResponse {
        manager: config.manager.to_string(),
        nois_proxy: config.nois_proxy.to_string(),
    })
}

#[cfg(test)]
mod tests {}
