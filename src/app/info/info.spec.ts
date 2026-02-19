import { ComponentFixture, TestBed } from '@angular/core/testing';

import { Info } from './info';
import { provideZonelessChangeDetection } from '@angular/core';

describe('Info', () => {
  let component: Info;
  let fixture: ComponentFixture<Info>;

  beforeEach(async () => {
    await TestBed.configureTestingModule({
      imports: [Info],
      providers: [provideZonelessChangeDetection()],
    })
    .compileComponents();

    fixture = TestBed.createComponent(Info);
    component = fixture.componentInstance;
    fixture.detectChanges();
  });

  it('should create', () => {
    expect(component).toBeTruthy();
  });
});
