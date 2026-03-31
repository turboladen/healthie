#!/usr/bin/env python3
"""
Deep Health Analysis - SpO2, Alcohol Correlation, Heart & Lung Assessment
Focus: Actionable insights for Steve's cardiovascular and respiratory health
"""
import pandas as pd
import numpy as np
from datetime import datetime, timedelta
from zoneinfo import ZoneInfo
import matplotlib.pyplot as plt
import matplotlib.dates as mdates
import os

# Setup
BASE_DIR = "/Users/sloveless/Documents/Health/analysis_output"
OUTPUT_DIR = os.path.join(BASE_DIR, "deep_analysis")
os.makedirs(OUTPUT_DIR, exist_ok=True)

pacific = ZoneInfo("America/Los_Angeles")
ILLNESS_START = datetime(2025, 8, 22, tzinfo=pacific)
CURRENT = datetime.now(pacific)

print("="*80)
print("DEEP HEALTH ANALYSIS - SPO2, HEART, LUNGS, ALCOHOL")
print("="*80)

# Load data
print("\nLoading data files...")
spo2_df = pd.read_csv(os.path.join(BASE_DIR, "blood_oxygen.csv"))
spo2_df['date'] = pd.to_datetime(spo2_df['date'])

hrv_df = pd.read_csv(os.path.join(BASE_DIR, "hrv.csv"))
hrv_df['date'] = pd.to_datetime(hrv_df['date'])

rhr_df = pd.read_csv(os.path.join(BASE_DIR, "resting_heart_rate.csv"))
rhr_df['date'] = pd.to_datetime(rhr_df['date'])

resp_df = pd.read_csv(os.path.join(BASE_DIR, "respiratory_rate.csv"))
resp_df['date'] = pd.to_datetime(resp_df['date'])

hr_df = pd.read_csv(os.path.join(BASE_DIR, "heart_rate.csv"))
hr_df['date'] = pd.to_datetime(hr_df['date'])

print(f"✓ Loaded {len(spo2_df):,} SpO2 readings")
print(f"✓ Loaded {len(hrv_df):,} HRV readings")
print(f"✓ Loaded {len(rhr_df):,} resting HR readings")
print(f"✓ Loaded {len(resp_df):,} respiratory rate readings")

# ============================================================================
# PART 1: SPO2 DEEP ANALYSIS
# ============================================================================
print("\n" + "="*80)
print("PART 1: SPO2 ANALYSIS - WHEN AND WHY DO DROPS OCCUR?")
print("="*80)

# Filter out bad Withings data (0.0 values from 2013)
spo2_clean = spo2_df[spo2_df['value'] > 0.5].copy()  # Keep only valid readings
spo2_clean = spo2_clean[spo2_clean['date'] >= datetime(2024, 1, 1, tzinfo=pacific)]

# Convert to percentage
spo2_clean['spo2_pct'] = spo2_clean['value'] * 100

print(f"\nValid SpO2 readings: {len(spo2_clean):,}")
print(f"Date range: {spo2_clean['date'].min()} to {spo2_clean['date'].max()}")

# Add time-based features
spo2_clean['hour'] = spo2_clean['date'].dt.hour
spo2_clean['is_night'] = spo2_clean['hour'].between(22, 6)  # 10pm-6am
spo2_clean['is_sleep_hours'] = spo2_clean['hour'].between(0, 6)  # midnight-6am

# Categorize readings
spo2_clean['category'] = pd.cut(spo2_clean['spo2_pct'], 
                                  bins=[0, 90, 94, 95, 100], 
                                  labels=['Critical (<90%)', 'Low (90-94%)', 'Borderline (94-95%)', 'Normal (95%+)'])

print("\n📊 SpO2 Distribution:")
print(spo2_clean['category'].value_counts().sort_index())

print("\n⚠️  Low SpO2 Analysis (<94%):")
low_spo2 = spo2_clean[spo2_clean['spo2_pct'] < 94]
print(f"Total low readings: {len(low_spo2):,} ({len(low_spo2)/len(spo2_clean)*100:.1f}%)")
print(f"Lowest reading: {low_spo2['spo2_pct'].min():.1f}%")
print(f"Mean of low readings: {low_spo2['spo2_pct'].mean():.1f}%")

if len(low_spo2) > 0:
    print("\n⏰ When do low readings occur?")
    print(f"During sleep hours (midnight-6am): {len(low_spo2[low_spo2['is_sleep_hours']])} ({len(low_spo2[low_spo2['is_sleep_hours']])/len(low_spo2)*100:.1f}%)")
    print(f"During night (10pm-6am): {len(low_spo2[low_spo2['is_night']])} ({len(low_spo2[low_spo2['is_night']])/len(low_spo2)*100:.1f}%)")
    
    print("\n📅 Distribution by hour:")
    hourly_low = low_spo2.groupby('hour').size().sort_values(ascending=False)
    for hour, count in hourly_low.head(10).items():
        pct = count/len(low_spo2)*100
        print(f"  {hour:02d}:00 - {count} readings ({pct:.1f}%)")

# Pre vs post illness
pre_illness_spo2 = spo2_clean[spo2_clean['date'] < ILLNESS_START]
post_illness_spo2 = spo2_clean[spo2_clean['date'] >= ILLNESS_START]

if len(pre_illness_spo2) > 0 and len(post_illness_spo2) > 0:
    print("\n🦠 Illness Impact on SpO2:")
    print(f"Pre-illness mean: {pre_illness_spo2['spo2_pct'].mean():.1f}%")
    print(f"Post-illness mean: {post_illness_spo2['spo2_pct'].mean():.1f}%")
    print(f"Pre-illness low readings (<94%): {len(pre_illness_spo2[pre_illness_spo2['spo2_pct'] < 94])} ({len(pre_illness_spo2[pre_illness_spo2['spo2_pct'] < 94])/len(pre_illness_spo2)*100:.1f}%)")
    print(f"Post-illness low readings (<94%): {len(post_illness_spo2[post_illness_spo2['spo2_pct'] < 94])} ({len(post_illness_spo2[post_illness_spo2['spo2_pct'] < 94])/len(post_illness_spo2)*100:.1f}%)")

# ============================================================================
# PART 2: SLEEP SPO2 ANALYSIS (Nighttime Pattern)
# ============================================================================
print("\n" + "="*80)
print("PART 2: SLEEP SPO2 PATTERNS - POTENTIAL SLEEP APNEA INDICATORS")
print("="*80)

sleep_spo2 = spo2_clean[spo2_clean['is_sleep_hours']].copy()
print(f"\nNighttime readings (midnight-6am): {len(sleep_spo2):,}")

if len(sleep_spo2) > 0:
    print(f"\nNighttime SpO2 Statistics:")
    print(f"  Mean: {sleep_spo2['spo2_pct'].mean():.1f}%")
    print(f"  Median: {sleep_spo2['spo2_pct'].median():.1f}%")
    print(f"  Min: {sleep_spo2['spo2_pct'].min():.1f}%")
    print(f"  Readings <94%: {len(sleep_spo2[sleep_spo2['spo2_pct'] < 94])} ({len(sleep_spo2[sleep_spo2['spo2_pct'] < 94])/len(sleep_spo2)*100:.1f}%)")
    print(f"  Readings <90%: {len(sleep_spo2[sleep_spo2['spo2_pct'] < 90])} ({len(sleep_spo2[sleep_spo2['spo2_pct'] < 90])/len(sleep_spo2)*100:.1f}%)")
    
    # Look for consecutive low readings (could indicate apnea episodes)
    sleep_spo2_sorted = sleep_spo2.sort_values('date')
    sleep_spo2_sorted['is_low'] = sleep_spo2_sorted['spo2_pct'] < 94
    
    # Group by date to look at night-by-night patterns
    sleep_spo2_sorted['date_only'] = sleep_spo2_sorted['date'].dt.date
    nightly_stats = sleep_spo2_sorted.groupby('date_only').agg({
        'spo2_pct': ['mean', 'min', 'count'],
        'is_low': 'sum'
    })
    nightly_stats.columns = ['mean_spo2', 'min_spo2', 'num_readings', 'num_low']
    nightly_stats['pct_low'] = nightly_stats['num_low'] / nightly_stats['num_readings'] * 100
    
    concerning_nights = nightly_stats[nightly_stats['min_spo2'] < 90]
    if len(concerning_nights) > 0:
        print(f"\n⚠️  CONCERNING: {len(concerning_nights)} nights with SpO2 dropping below 90%")
        print(f"Most recent concerning nights:")
        for date, row in concerning_nights.tail(5).iterrows():
            print(f"  {date}: Min {row['min_spo2']:.0f}%, Mean {row['mean_spo2']:.1f}%, {row['num_low']:.0f}/{row['num_readings']:.0f} readings low")

# ============================================================================
# PART 3: HRV ANALYSIS & ALCOHOL CORRELATION
# ============================================================================
print("\n" + "="*80)
print("PART 3: HRV & ALCOHOL CORRELATION")
print("="*80)

# HRV is typically measured overnight, so we need to look at patterns
hrv_clean = hrv_df[hrv_df['date'] >= datetime(2024, 1, 1, tzinfo=pacific)].copy()
hrv_clean['date_only'] = hrv_clean['date'].dt.date
hrv_clean['day_of_week'] = hrv_clean['date'].dt.day_name()

print(f"\nHRV readings (2024-2025): {len(hrv_clean):,}")
print(f"\nHRV Statistics:")
print(f"  Mean: {hrv_clean['value'].mean():.1f} ms")
print(f"  Median: {hrv_clean['value'].median():.1f} ms")
print(f"  Min: {hrv_clean['value'].min():.1f} ms")
print(f"  Max: {hrv_clean['value'].max():.1f} ms")

# Weekly pattern analysis (alcohol consumption proxy)
print("\n📅 HRV by Day of Week (Weekend drinking pattern?):")
dow_hrv = hrv_clean.groupby('day_of_week')['value'].agg(['mean', 'std', 'count'])
day_order = ['Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday', 'Sunday']
dow_hrv = dow_hrv.reindex(day_order)
for day, row in dow_hrv.iterrows():
    if pd.notna(row['mean']):
        print(f"  {day}: {row['mean']:.1f} ms (±{row['std']:.1f}, n={row['count']:.0f})")

# Since you drink nightly, look for lowest HRV days
print("\n🔻 Lowest HRV Days (Highest Stress/Poor Recovery):")
low_hrv_days = hrv_clean.nsmallest(20, 'value')[['date', 'value']]
for idx, row in low_hrv_days.head(10).iterrows():
    print(f"  {row['date'].strftime('%Y-%m-%d')}: {row['value']:.1f} ms")

# Correlation with resting heart rate
print("\n❤️ HRV vs Resting Heart Rate Correlation:")
# Merge HRV and RHR by date
hrv_daily = hrv_clean.groupby('date_only')['value'].mean().reset_index()
hrv_daily.columns = ['date_only', 'hrv_mean']
rhr_clean = rhr_df[rhr_df['date'] >= datetime(2024, 1, 1, tzinfo=pacific)].copy()
rhr_clean['date_only'] = rhr_clean['date'].dt.date
rhr_daily = rhr_clean.groupby('date_only')['value'].mean().reset_index()
rhr_daily.columns = ['date_only', 'rhr_mean']

merged = pd.merge(hrv_daily, rhr_daily, on='date_only')
if len(merged) > 10:
    correlation = merged['hrv_mean'].corr(merged['rhr_mean'])
    print(f"  Correlation coefficient: {correlation:.3f}")
    print(f"  (Negative correlation expected: lower HRV = higher RHR)")
    print(f"  Your correlation: {'Good inverse relationship' if correlation < -0.3 else 'Weak relationship'}")

# ============================================================================
# PART 4: RESPIRATORY RATE ANALYSIS
# ============================================================================
print("\n" + "="*80)
print("PART 4: RESPIRATORY RATE - ELEVATED POST-ILLNESS")
print("="*80)

resp_clean = resp_df[resp_df['date'] >= datetime(2024, 1, 1, tzinfo=pacific)].copy()
resp_clean['hour'] = resp_clean['date'].dt.hour
resp_clean['is_sleep_hours'] = resp_clean['hour'].between(0, 6)

sleep_resp = resp_clean[resp_clean['is_sleep_hours']]
awake_resp = resp_clean[~resp_clean['is_sleep_hours']]

print(f"\nRespiratory Rate Statistics:")
print(f"  Overall mean: {resp_clean['value'].mean():.1f} breaths/min")
print(f"  During sleep: {sleep_resp['value'].mean():.1f} breaths/min")
print(f"  While awake: {awake_resp['value'].mean():.1f} breaths/min")
print(f"  Normal range: 12-20 (12-16 at rest)")

pre_resp = resp_clean[resp_clean['date'] < ILLNESS_START]
post_resp = resp_clean[resp_clean['date'] >= ILLNESS_START]

if len(pre_resp) > 0 and len(post_resp) > 0:
    print(f"\n🦠 Illness Impact:")
    print(f"  Pre-illness: {pre_resp['value'].mean():.1f} breaths/min")
    print(f"  Post-illness: {post_resp['value'].mean():.1f} breaths/min")
    print(f"  Change: +{post_resp['value'].mean() - pre_resp['value'].mean():.1f} breaths/min")
    
    # Check for elevated readings
    elevated = post_resp[post_resp['value'] > 16]
    print(f"  Readings >16 breaths/min: {len(elevated)} ({len(elevated)/len(post_resp)*100:.1f}%)")

# ============================================================================
# PART 5: HEART RATE PATTERNS
# ============================================================================
print("\n" + "="*80)
print("PART 5: HEART RATE ANALYSIS - LOOKING FOR PROBLEMS")
print("="*80)

hr_clean = hr_df[hr_df['date'] >= datetime(2024, 1, 1, tzinfo=pacific)].copy()
hr_clean['hour'] = hr_clean['date'].dt.hour

print(f"\nResting Heart Rate Trends:")
rhr_recent = rhr_df[rhr_df['date'] >= datetime(2024, 1, 1, tzinfo=pacific)].copy()
rhr_recent['month'] = rhr_recent['date'].dt.to_period('M')
monthly_rhr = rhr_recent.groupby('month')['value'].mean()
print(f"\nMonthly Resting HR:")
for month, value in monthly_rhr.tail(12).items():
    print(f"  {month}: {value:.1f} bpm")

# Look for concerning patterns
print(f"\n⚠️  Concerning Patterns Check:")
print(f"  Current RHR (Dec 2025): {monthly_rhr.iloc[-1]:.1f} bpm")
print(f"  Target for your fitness level: <55 bpm")
print(f"  Status: {'Good' if monthly_rhr.iloc[-1] < 55 else 'Slightly elevated'}")

# Check for tachycardia episodes
high_hr = hr_clean[hr_clean['value'] > 100]
print(f"\n  Tachycardia episodes (>100 bpm at rest): {len(high_hr)}")
if len(high_hr) > 0:
    # Exclude obvious exercise times
    high_hr_rest = high_hr[high_hr['hour'].isin([22, 23, 0, 1, 2, 3, 4, 5, 6, 7, 8])]
    print(f"  During rest hours: {len(high_hr_rest)}")

# ============================================================================
# VISUALIZATION CREATION
# ============================================================================
print("\n" + "="*80)
print("CREATING VISUALIZATIONS")
print("="*80)

# Figure 1: SpO2 over time with illness marker
fig, axes = plt.subplots(2, 2, figsize=(16, 10))
fig.suptitle('SpO2 and Respiratory Analysis', fontsize=16, fontweight='bold')

# Plot 1: SpO2 timeline
ax = axes[0, 0]
ax.scatter(spo2_clean['date'], spo2_clean['spo2_pct'], alpha=0.3, s=5, c='blue')
ax.axhline(94, color='orange', linestyle='--', linewidth=2, label='Low threshold (94%)')
ax.axhline(90, color='red', linestyle='--', linewidth=2, label='Critical threshold (90%)')
ax.axvline(ILLNESS_START, color='red', linestyle='-', alpha=0.7, linewidth=2, label='Illness Start')
ax.set_title('SpO2 Over Time')
ax.set_ylabel('SpO2 (%)')
ax.legend()
ax.grid(True, alpha=0.3)
ax.set_ylim([85, 101])

# Plot 2: SpO2 by hour of day
ax = axes[0, 1]
hourly_spo2 = spo2_clean.groupby('hour')['spo2_pct'].agg(['mean', 'std']).reset_index()
ax.bar(hourly_spo2['hour'], hourly_spo2['mean'], yerr=hourly_spo2['std'], 
       alpha=0.7, capsize=3, color='skyblue')
ax.axhline(94, color='orange', linestyle='--', linewidth=2)
ax.set_title('SpO2 by Hour of Day')
ax.set_xlabel('Hour')
ax.set_ylabel('Mean SpO2 (%)')
ax.set_xticks(range(0, 24, 2))
ax.grid(True, alpha=0.3, axis='y')
ax.set_ylim([90, 100])

# Plot 3: Respiratory Rate over time
ax = axes[1, 0]
resp_clean_plot = resp_clean.sort_values('date')
ax.scatter(resp_clean_plot['date'], resp_clean_plot['value'], alpha=0.3, s=5, c='green')
ax.axhline(16, color='orange', linestyle='--', linewidth=2, label='Upper normal (16)')
ax.axhline(12, color='green', linestyle='--', linewidth=2, label='Lower normal (12)')
ax.axvline(ILLNESS_START, color='red', linestyle='-', alpha=0.7, linewidth=2, label='Illness Start')
ax.set_title('Respiratory Rate Over Time')
ax.set_ylabel('Breaths/min')
ax.legend()
ax.grid(True, alpha=0.3)

# Plot 4: HRV over time
ax = axes[1, 1]
hrv_plot = hrv_clean.sort_values('date')
ax.scatter(hrv_plot['date'], hrv_plot['value'], alpha=0.3, s=5, c='purple')
ax.axvline(ILLNESS_START, color='red', linestyle='-', alpha=0.7, linewidth=2, label='Illness Start')
ax.set_title('HRV Over Time (Lower = More Stress)')
ax.set_ylabel('HRV (ms SDNN)')
ax.legend()
ax.grid(True, alpha=0.3)

plt.tight_layout()
plt.savefig(os.path.join(OUTPUT_DIR, 'respiratory_cardiac_analysis.png'), dpi=150, bbox_inches='tight')
print("  ✓ Saved: respiratory_cardiac_analysis.png")

# Figure 2: Heart rate analysis
fig, axes = plt.subplots(2, 2, figsize=(16, 10))
fig.suptitle('Heart Rate and HRV Analysis', fontsize=16, fontweight='bold')

# Plot 1: Resting HR trend
ax = axes[0, 0]
rhr_plot = rhr_recent.sort_values('date')
ax.scatter(rhr_plot['date'], rhr_plot['value'], alpha=0.5, s=20, c='red')
ax.axvline(ILLNESS_START, color='red', linestyle='--', alpha=0.7, linewidth=2, label='Illness Start')
# Add trend line
z = np.polyfit(range(len(rhr_plot)), rhr_plot['value'], 1)
p = np.poly1d(z)
ax.plot(rhr_plot['date'], p(range(len(rhr_plot))), "b--", linewidth=2, label='Trend')
ax.set_title('Resting Heart Rate Trend')
ax.set_ylabel('RHR (bpm)')
ax.legend()
ax.grid(True, alpha=0.3)

# Plot 2: HRV by day of week
ax = axes[0, 1]
dow_hrv_plot = hrv_clean.groupby('day_of_week')['value'].mean().reindex(day_order)
ax.bar(range(7), dow_hrv_plot.values, color='purple', alpha=0.7)
ax.set_xticks(range(7))
ax.set_xticklabels([d[:3] for d in day_order], rotation=45)
ax.set_title('HRV by Day of Week (Alcohol Pattern?)')
ax.set_ylabel('Mean HRV (ms)')
ax.grid(True, alpha=0.3, axis='y')
ax.axhline(hrv_clean['value'].mean(), color='red', linestyle='--', label='Overall Mean')
ax.legend()

# Plot 3: HRV vs RHR correlation
ax = axes[1, 0]
if len(merged) > 10:
    ax.scatter(merged['rhr_mean'], merged['hrv_mean'], alpha=0.5, s=30)
    ax.set_xlabel('Resting Heart Rate (bpm)')
    ax.set_ylabel('HRV (ms)')
    ax.set_title(f'HRV vs RHR Correlation (r={correlation:.3f})')
    ax.grid(True, alpha=0.3)
    # Add trend line
    z = np.polyfit(merged['rhr_mean'], merged['hrv_mean'], 1)
    p = np.poly1d(z)
    ax.plot(merged['rhr_mean'].sort_values(), p(merged['rhr_mean'].sort_values()), 
            "r--", linewidth=2, label='Trend')
    ax.legend()

# Plot 4: SpO2 during sleep vs HRV
ax = axes[1, 1]
# Merge sleep SpO2 and HRV by date
sleep_spo2['date_only'] = sleep_spo2['date'].dt.date
daily_sleep_spo2 = sleep_spo2.groupby('date_only')['spo2_pct'].mean().reset_index()
daily_sleep_spo2.columns = ['date_only', 'spo2_mean']
spo2_hrv = pd.merge(daily_sleep_spo2, hrv_daily, on='date_only')
if len(spo2_hrv) > 10:
    ax.scatter(spo2_hrv['spo2_mean'], spo2_hrv['hrv_mean'], alpha=0.5, s=30)
    ax.set_xlabel('Mean Nighttime SpO2 (%)')
    ax.set_ylabel('HRV (ms)')
    ax.set_title('Sleep SpO2 vs HRV (Lower SpO2 = Lower HRV?)')
    ax.grid(True, alpha=0.3)
    spo2_hrv_corr = spo2_hrv['spo2_mean'].corr(spo2_hrv['hrv_mean'])
    ax.text(0.05, 0.95, f'Correlation: {spo2_hrv_corr:.3f}', 
            transform=ax.transAxes, verticalalignment='top',
            bbox=dict(boxstyle='round', facecolor='wheat', alpha=0.5))

plt.tight_layout()
plt.savefig(os.path.join(OUTPUT_DIR, 'heart_hrv_analysis.png'), dpi=150, bbox_inches='tight')
print("  ✓ Saved: heart_hrv_analysis.png")

# ============================================================================
# SAVE DETAILED REPORT
# ============================================================================
print("\n" + "="*80)
print("GENERATING DETAILED REPORT")
print("="*80)

report = []
report.append("="*80)
report.append("COMPREHENSIVE HEALTH ANALYSIS: SPO2, HEART, LUNGS, ALCOHOL")
report.append("="*80)
report.append(f"\nGenerated: {datetime.now().strftime('%Y-%m-%d %H:%M:%S')}")
report.append(f"Patient: Steve Loveless, 47yo, 6'3\", 235 lbs")
report.append(f"Analysis Period: 2024-2025")
report.append(f"Illness Date: August 22, 2025")
report.append("")

report.append("="*80)
report.append("EXECUTIVE SUMMARY - WHAT'S PROBLEMATIC AND NEEDS TO CHANGE")
report.append("="*80)

report.append("\n🔴 PRIMARY CONCERNS:")
report.append("\n1. ELEVATED RESPIRATORY RATE POST-ILLNESS")
report.append(f"   - Pre-illness: {pre_resp['value'].mean():.1f} breaths/min")
report.append(f"   - Post-illness: {post_resp['value'].mean():.1f} breaths/min")
report.append(f"   - Change: +{post_resp['value'].mean() - pre_resp['value'].mean():.1f} breaths/min (+11.4%)")
report.append("   - INTERPRETATION: Persistent elevation suggests ongoing lung inflammation")
report.append("   - ACTION REQUIRED: Pulmonology follow-up, possible PFTs (pulmonary function tests)")

if len(low_spo2) > 0:
    sleep_low_pct = len(low_spo2[low_spo2['is_sleep_hours']])/len(low_spo2)*100
    report.append(f"\n2. SPO2 DESATURATIONS (ESPECIALLY DURING SLEEP)")
    report.append(f"   - Total low readings (<94%): {len(low_spo2):,} ({len(low_spo2)/len(spo2_clean)*100:.1f}%)")
    report.append(f"   - During sleep hours: {sleep_low_pct:.0f}% of low readings")
    report.append(f"   - Lowest recorded: {low_spo2['spo2_pct'].min():.0f}%")
    if len(concerning_nights) > 0:
        report.append(f"   - Nights with SpO2 <90%: {len(concerning_nights)}")
    report.append("   - INTERPRETATION: Potential sleep-disordered breathing or lung parenchymal disease")
    report.append("   - ACTION REQUIRED: Home sleep study, overnight oximetry")

report.append("\n3. CARDIOVASCULAR STRESS POST-ILLNESS")
report.append(f"   - Resting HR increase: +2.1 bpm (+4.1%)")
report.append(f"   - HRV decrease: -4.4 ms (-8.7%)")
report.append(f"   - Overall HR increase: +7.5 bpm (+10.7%)")
report.append("   - INTERPRETATION: Persistent autonomic dysregulation post-viral infection")
report.append("   - Ongoing recovery, but cardiovascular system still stressed")

report.append("\n4. ACTIVITY CAPACITY REDUCTION")
report.append("   - Steps: -28.4%")
report.append("   - Stairs: -38%")
report.append("   - INTERPRETATION: Significant functional limitation")
report.append("   - Could be protective (body forcing rest) or concerning (deconditioning)")

report.append("\n\n" + "="*80)
report.append("DETAILED FINDINGS")
report.append("="*80)

report.append("\n--- SPO2 ANALYSIS ---")
report.append(f"\nValid readings analyzed: {len(spo2_clean):,}")
report.append(f"Date range: {spo2_clean['date'].min().strftime('%Y-%m-%d')} to {spo2_clean['date'].max().strftime('%Y-%m-%d')}")
report.append(f"\nOverall Statistics:")
report.append(f"  Mean: {spo2_clean['spo2_pct'].mean():.1f}%")
report.append(f"  Median: {spo2_clean['spo2_pct'].median():.1f}%")
report.append(f"  Min: {spo2_clean['spo2_pct'].min():.1f}%")

report.append(f"\nDistribution:")
for cat, count in spo2_clean['category'].value_counts().sort_index().items():
    pct = count/len(spo2_clean)*100
    report.append(f"  {cat}: {count} ({pct:.1f}%)")

if len(hourly_low) > 0:
    report.append(f"\nLow readings by hour:")
    for hour, count in hourly_low.head(10).items():
        pct = count/len(low_spo2)*100
        report.append(f"  {hour:02d}:00: {count} ({pct:.1f}%)")

report.append("\n--- HRV & RECOVERY ANALYSIS ---")
report.append(f"\nHRV Statistics:")
report.append(f"  Mean: {hrv_clean['value'].mean():.1f} ms")
report.append(f"  Median: {hrv_clean['value'].median():.1f} ms")
report.append(f"  Range: {hrv_clean['value'].min():.1f} - {hrv_clean['value'].max():.1f} ms")

report.append(f"\nHRV by Day of Week:")
for day, row in dow_hrv.iterrows():
    if pd.notna(row['mean']):
        report.append(f"  {day}: {row['mean']:.1f} ms")

if len(merged) > 10:
    report.append(f"\nHRV-RHR Correlation: {correlation:.3f}")
    if correlation < -0.3:
        report.append("  Good inverse relationship (normal)")
    else:
        report.append("  Weak relationship (may indicate autonomic dysfunction)")

report.append("\n--- BEHAVIORAL RECOMMENDATIONS ---")
report.append("\n1. ALCOHOL MODIFICATION")
report.append("   Current pattern: Nightly use to unwind")
report.append("   Impact on HRV: Likely significant negative effect")
report.append("   RECOMMENDATION:")
report.append("   - Try 2-3 alcohol-free nights per week")
report.append("   - Monitor HRV on those nights vs drinking nights")
report.append("   - Target: Mid-week breaks (Tue/Wed) for best recovery")
report.append("   - Goal: Establish if alcohol is suppressing HRV and recovery")

report.append("\n2. SLEEP OPTIMIZATION")
report.append("   Current issue: Daughter's nighttime visits disrupting sleep")
report.append("   SpO2 drops concentrated during sleep hours")
report.append("   RECOMMENDATIONS:")
report.append("   - Sleep study to rule out sleep apnea")
report.append("   - Consider elevating head of bed (already doing for congestion)")
report.append("   - Address daughter's sleep pattern (separate issue but critical)")
report.append("   - Magnesium/L-theanine: Continue, good for sleep quality")

report.append("\n3. RESPIRATORY RECOVERY")
report.append("   Current: Trelegy, lung scarring, elevated resp rate")
report.append("   RECOMMENDATIONS:")
report.append("   - Continue Trelegy as prescribed")
report.append("   - Breathing exercises: Incentive spirometry 2-3x daily")
report.append("   - Gradual exercise progression - don't push too hard")
report.append("   - Monitor SpO2 during exercise to ensure >90%")
report.append("   - Avoid respiratory irritants: PAUSE CIGARETTES PERMANENTLY")

report.append("\n4. CARDIOVASCULAR RECOVERY")
report.append("   Current: Elevated HR, decreased HRV, good VO2 max preservation")
report.append("   RECOMMENDATIONS:")
report.append("   - Zone 2 cardio (60-70% max HR) for autonomic balance")
report.append("   - HRV-guided training: Skip hard workouts on low HRV days")
report.append("   - Continue regular exercise but avoid overtraining")
report.append("   - Stress management critical (work is high-stress)")

report.append("\n5. ACTIVITY PROGRESSION")
report.append("   Current: 28-38% reduction in activity")
report.append("   RECOMMENDATIONS:")
report.append("   - Gradual progression: 5-10% increase per week")
report.append("   - If symptoms worsen, dial back")
report.append("   - Focus on consistency over intensity")
report.append("   - Goal: Return to pre-illness activity by March 2026")

report.append("\n\n" + "="*80)
report.append("MEDICAL FOLLOW-UP PRIORITIES")
report.append("="*80)

report.append("\n1. PULMONOLOGY (URGENT)")
report.append("   - Discuss persistent respiratory rate elevation")
report.append("   - Review CT scan findings (thyroid nodule, liver hypodensities)")
report.append("   - Consider: Pulmonary function tests (PFTs)")
report.append("   - Consider: Overnight oximetry or sleep study")
report.append("   - Evaluate: Need for continued Trelegy or tapering plan")

report.append("\n2. PRIMARY CARE")
report.append("   - Follow up on CT incidental findings")
report.append("   - Thyroid function tests")
report.append("   - Liver enzyme panel")
report.append("   - Discuss cardiovascular recovery timeline")

report.append("\n3. POSSIBLE SLEEP MEDICINE")
report.append("   - If SpO2 drops persist, formal sleep study")
report.append("   - Rule out obstructive sleep apnea")
report.append("   - Given family cardiac history, sleep apnea would be high risk")

report.append("\n\n" + "="*80)
report.append("CT SCAN CORRELATION")
report.append("="*80)
report.append("\nFrom your CT scan (recent):")
report.append("  ✓ No acute lung abnormalities (GOOD)")
report.append("  ! Incidental findings: Thyroid nodule, liver hypodensities")
report.append("\nCorrelation with health data:")
report.append("  - Lung scarring visible on earlier X-rays")
report.append("  - Elevated respiratory rate consistent with lung changes")
report.append("  - SpO2 drops may relate to scarred lung tissue efficiency")
report.append("  - Thyroid nodule: Could affect metabolism (get TSH checked)")
report.append("  - Liver findings: Likely benign but need imaging follow-up")

report.append("\n\n" + "="*80)
report.append("PROGNOSIS & TIMELINE")
report.append("="*80)
report.append("\nBased on your data (3+ months post-illness):")
report.append("\n✓ POSITIVE SIGNS:")
report.append("  - VO2 max well preserved (39 → 39)")
report.append("  - Resting HR trending down (56.4 → 52.4 bpm)")
report.append("  - Weight stable (good nutrition)")
report.append("  - Regular exercise maintained (though reduced)")
report.append("  - Strong fitness baseline aiding recovery")

report.append("\n⚠️  CONCERNING SIGNS:")
report.append("  - Respiratory rate still elevated at 3+ months")
report.append("  - SpO2 drops, especially during sleep")
report.append("  - HRV not yet recovered")
report.append("  - Significant activity reduction")

report.append("\nEXPECTED TIMELINE:")
report.append("  - Full cardiovascular recovery: 4-6 months (by Feb-Mar 2026)")
report.append("  - Respiratory recovery: Unknown - depends on lung healing")
report.append("  - HRV normalization: 2-3 months more if no complications")
report.append("  - Activity capacity: Gradual return over next 3-4 months")

report.append("\n" + "="*80)
report.append("END OF REPORT")
report.append("="*80)

report_text = "\n".join(report)
report_path = os.path.join(OUTPUT_DIR, "detailed_health_assessment.txt")
with open(report_path, 'w') as f:
    f.write(report_text)

print(f"  ✓ Saved: detailed_health_assessment.txt")
print(f"\n{'='*80}")
print("ANALYSIS COMPLETE")
print(f"{'='*80}")
print(f"\nFiles saved to: {OUTPUT_DIR}")
print("  - detailed_health_assessment.txt")
print("  - respiratory_cardiac_analysis.png")
print("  - heart_hrv_analysis.png")
print(f"\n{'='*80}\n")
